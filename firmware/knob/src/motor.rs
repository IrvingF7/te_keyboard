//! Six MCPWM outputs for the TMC6300. All unsafe calls are confined to this
//! hardware boundary; virtual-detent arithmetic lives in the tested core crate.
use board_support::{check, input_pullup, is_low, micros, output, sys};
use esp_idf_svc::hal::interrupt::IsrCriticalSection;
use keyboard_core::motor::phase_ticks;
use std::{cell::UnsafeCell, ptr};

const PINS: [[i32; 2]; 3] = [[6, 5], [1, 3], [0, 4]];
pub fn safe_pins() -> anyhow::Result<()> {
    for pin in PINS.into_iter().flatten() {
        output(pin, false)?;
    }
    Ok(())
}
struct Gates {
    generators: [[sys::mcpwm_gen_handle_t; 2]; 3],
    comparators: [sys::mcpwm_cmpr_handle_t; 3],
    enabled: bool,
    timed_out: bool,
    last_command: u32,
    cutoff_age: u32,
}
impl Gates {
    fn off(&mut self) {
        // Force is BEFORE low-side inversion, so logical 1 on low-side gives
        // physical LOW. Forcing logical 0 on all six would energize low sides.
        for phase in self.generators {
            for (side, generator) in phase.into_iter().enumerate() {
                unsafe {
                    sys::mcpwm_generator_set_force_level(generator, side as i32, true);
                }
            }
        }
        self.enabled = false;
    }
}
struct Shared {
    lock: IsrCriticalSection,
    gates: UnsafeCell<Gates>,
}
// SAFETY: every access to Gates holds the SAME ISR-safe critical section.
// The allocation and MCPWM handles remain valid for the firmware's lifetime.
unsafe impl Sync for Shared {}
impl Shared {
    fn with<R>(&self, f: impl FnOnce(&mut Gates) -> R) -> R {
        let _guard = self.lock.enter();
        // No references escape this closure, and an ISR cannot preempt while
        // we hold the guard. A task mutex would NOT be safe in the interrupt.
        f(unsafe { &mut *self.gates.get() })
    }
}
pub struct Motor {
    shared: &'static Shared,
}
impl Motor {
    pub fn new() -> anyhow::Result<Self> {
        safe_pins()?;
        input_pullup(2)?;
        let config = sys::mcpwm_timer_config_t {
            group_id: 0,
            clk_src: sys::soc_periph_mcpwm_timer_clk_src_t_MCPWM_TIMER_CLK_SRC_DEFAULT,
            resolution_hz: 10_000_000,
            period_ticks: 500,
            count_mode: sys::mcpwm_timer_count_mode_t_MCPWM_TIMER_COUNT_MODE_UP,
            ..Default::default()
        };
        let mut timer = ptr::null_mut();
        check(unsafe { sys::mcpwm_new_timer(&config, &mut timer) })?;
        let mut generators = [[ptr::null_mut(); 2]; 3];
        let mut comparators = [ptr::null_mut(); 3];
        for phase in 0..3 {
            let mut op = ptr::null_mut();
            let op_config = sys::mcpwm_operator_config_t {
                group_id: 0,
                ..Default::default()
            };
            check(unsafe { sys::mcpwm_new_operator(&op_config, &mut op) })?;
            check(unsafe { sys::mcpwm_operator_connect_timer(op, timer) })?;
            let mut cmp_config = sys::mcpwm_comparator_config_t::default();
            cmp_config.flags.set_update_cmp_on_tez(1);
            check(unsafe { sys::mcpwm_new_comparator(op, &cmp_config, &mut comparators[phase]) })?;
            check(unsafe { sys::mcpwm_comparator_set_compare_value(comparators[phase], 250) })?;
            for side in 0..2 {
                let gen_config = sys::mcpwm_generator_config_t {
                    gen_gpio_num: PINS[phase][side],
                    ..Default::default()
                };
                check(unsafe {
                    sys::mcpwm_new_generator(op, &gen_config, &mut generators[phase][side])
                })?;
                let generator = generators[phase][side];
                check(unsafe {
                    sys::mcpwm_generator_set_action_on_timer_event(
                        generator,
                        sys::mcpwm_gen_timer_event_action_t {
                            direction: sys::mcpwm_timer_direction_t_MCPWM_TIMER_DIRECTION_UP,
                            event: sys::mcpwm_timer_event_t_MCPWM_TIMER_EVENT_EMPTY,
                            action: sys::mcpwm_generator_action_t_MCPWM_GEN_ACTION_HIGH,
                        },
                    )
                })?;
                check(unsafe {
                    sys::mcpwm_generator_set_action_on_compare_event(
                        generator,
                        sys::mcpwm_gen_compare_event_action_t {
                            direction: sys::mcpwm_timer_direction_t_MCPWM_TIMER_DIRECTION_UP,
                            comparator: comparators[phase],
                            action: sys::mcpwm_generator_action_t_MCPWM_GEN_ACTION_LOW,
                        },
                    )
                })?;
                let mut dead_time = sys::mcpwm_dead_time_config_t::default();
                if side == 0 {
                    dead_time.posedge_delay_ticks = 5;
                } else {
                    dead_time.negedge_delay_ticks = 5;
                    dead_time.flags.set_invert_output(1);
                }
                // 20 kHz complementary switching with 0.5 us of both sides off.
                check(unsafe {
                    sys::mcpwm_generator_set_dead_time(generator, generator, &dead_time)
                })?;
                check(unsafe {
                    sys::mcpwm_generator_set_force_level(generator, side as i32, true)
                })?;
            }
        }
        check(unsafe { sys::mcpwm_timer_enable(timer) })?;
        check(unsafe {
            sys::mcpwm_timer_start_stop(
                timer,
                sys::mcpwm_timer_start_stop_cmd_t_MCPWM_TIMER_START_NO_STOP,
            )
        })?;
        let shared = Box::leak(Box::new(Shared {
            lock: IsrCriticalSection::new(),
            gates: UnsafeCell::new(Gates {
                generators,
                comparators,
                enabled: false,
                timed_out: false,
                last_command: 0,
                cutoff_age: 0,
            }),
        }));
        let mut watchdog = ptr::null_mut();
        let config = sys::gptimer_config_t {
            clk_src: sys::soc_periph_gptimer_clk_src_t_GPTIMER_CLK_SRC_DEFAULT,
            direction: sys::gptimer_count_direction_t_GPTIMER_COUNT_UP,
            resolution_hz: 1_000_000,
            ..Default::default()
        };
        check(unsafe { sys::gptimer_new_timer(&config, &mut watchdog) })?;
        let callbacks = sys::gptimer_event_callbacks_t {
            on_alarm: Some(watchdog_alarm),
        };
        check(unsafe {
            sys::gptimer_register_event_callbacks(
                watchdog,
                &callbacks,
                (shared as *mut Shared).cast(),
            )
        })?;
        let mut alarm = sys::gptimer_alarm_config_t {
            alarm_count: 1000,
            ..Default::default()
        };
        alarm.flags.set_auto_reload_on_alarm(1);
        check(unsafe { sys::gptimer_set_alarm_action(watchdog, &alarm) })?;
        check(unsafe { sys::gptimer_enable(watchdog) })?;
        check(unsafe { sys::gptimer_start(watchdog) })?;
        Ok(Self { shared })
    }
    pub fn fault(&self) -> bool {
        !is_low(2)
    }
    pub fn cutoff_age(&self) -> u32 {
        self.shared.with(|g| g.cutoff_age)
    }
    pub fn timed_out(&self) -> bool {
        self.shared.with(|g| g.timed_out)
    }
    pub fn coast(&self) {
        self.shared.with(Gates::off);
    }
    pub fn torque(&self, q: f32, angle: f32) -> anyhow::Result<()> {
        anyhow::ensure!(
            q.is_finite() && angle.is_finite() && q.abs() <= 1.0,
            "invalid motor command"
        );
        let ticks = phase_ticks(q, angle);
        let was_enabled = self.shared.with(|g| -> Result<bool, sys::EspError> {
            if g.timed_out {
                return Ok(true);
            }
            for (cmp, value) in g.comparators.into_iter().zip(ticks) {
                check(unsafe { sys::mcpwm_comparator_set_compare_value(cmp, value) })?;
            }
            Ok(g.enabled)
        })?;
        // Give shadow compare registers a full PWM cycle to latch. Interrupts
        // remain enabled so this wait cannot prevent the independent cutoff.
        if !was_enabled {
            unsafe {
                sys::esp_rom_delay_us(60);
            }
        }
        self.shared.with(|g| -> Result<(), sys::EspError> {
            // A timeout occurring during the wait wins permanently.
            if !g.timed_out {
                if !g.enabled {
                    for generator in g.generators.into_iter().flatten() {
                        check(unsafe {
                            sys::mcpwm_generator_set_force_level(generator, -1, true)
                        })?;
                    }
                    g.enabled = true;
                }
                g.last_command = micros();
            }
            Ok(())
        })?;
        Ok(())
    }
}
impl Drop for Motor {
    fn drop(&mut self) {
        self.coast();
    }
}

unsafe extern "C" fn watchdog_alarm(
    _: sys::gptimer_handle_t,
    _: *const sys::gptimer_alarm_event_data_t,
    context: *mut core::ffi::c_void,
) -> bool {
    // SAFETY: context is a leaked Shared allocated before registering this ISR.
    // No logging, allocation, sleeping, or task mutexes are allowed here.
    let shared = unsafe { &*context.cast::<Shared>() };
    shared.with(|g| {
        let age = micros().wrapping_sub(g.last_command);
        if g.enabled && age >= 20_000 {
            g.off();
            g.cutoff_age = age;
            g.timed_out = true;
        }
    });
    false
}
