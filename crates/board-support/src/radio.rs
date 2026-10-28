use crate::{check, sys};
use esp_idf_svc::{
    espnow::{EspNow, PeerInfo},
    eventloop::EspSystemEventLoop,
    hal::modem::Modem,
    nvs::EspDefaultNvsPartition,
    wifi::{AuthMethod, ClientConfiguration, Configuration, EspWifi},
};
use keyboard_core::protocol::{LinkState, Packet, Role};
use std::sync::mpsc::{sync_channel, Receiver};

pub struct Radio {
    // Drop ESP-NOW before Wi-Fi. These owned handles keep both services alive.
    now: EspNow<'static>,
    _wifi: EspWifi<'static>,
    _nvs: EspDefaultNvsPartition,
    inbox: Receiver<Packet>,
    peer: [u8; 6],
    pub link: LinkState,
    pub sent: u32,
    pub errors: u32,
}
impl Radio {
    pub fn new(modem: Modem, role: Role) -> anyhow::Result<Self> {
        // PHY calibration uses NVS even when Wi-Fi configuration persistence is
        // disabled. Initialize it BEFORE the motor starts, so any calibration
        // writes happen while all motor outputs are still off.
        let nvs = EspDefaultNvsPartition::take()?;
        let mut wifi = EspWifi::new(modem, EspSystemEventLoop::take()?, None)?;
        wifi.set_configuration(&Configuration::Client(ClientConfiguration {
            channel: Some(1),
            auth_method: AuthMethod::None,
            ..Default::default()
        }))?;
        wifi.start()?; // Start the radio without connecting to a router.
        check(unsafe {
            sys::esp_wifi_set_channel(1, sys::wifi_second_chan_t_WIFI_SECOND_CHAN_NONE)
        })?;
        let now = EspNow::take()?; // Also disables modem sleep for this bench link.
        let peer = role.peer().mac();
        now.add_peer(PeerInfo {
            peer_addr: peer,
            channel: 1,
            ifidx: sys::wifi_interface_t_WIFI_IF_STA,
            encrypt: false,
            ..Default::default()
        })?;
        let (sender, inbox) = sync_channel(8);
        now.register_recv_cb(move |info, bytes| {
            // Borrowed callback bytes stop being valid when this call returns.
            // Decode into an owned Packet, then pass a COPY to the application.
            // A full inbox drops a heartbeat instead of waiting in the Wi-Fi task.
            if info.src_addr == &peer {
                if let Some(packet) = Packet::decode(bytes, role.peer()) {
                    let _ = sender.try_send(packet);
                }
            }
        })?;
        let session = loop {
            let s = unsafe { sys::esp_random() };
            if s != 0 {
                break s;
            }
        };
        println!("ESP-NOW Rust role={role:?} peer={peer:02x?} channel=1 session={session}");
        Ok(Self {
            now,
            _wifi: wifi,
            _nvs: nvs,
            inbox,
            peer,
            link: LinkState::new(role, session),
            sent: 0,
            errors: 0,
        })
    }
    pub fn tick(&mut self, time: u32, angle: u16, steps: i32, flags: u8) {
        while let Ok(packet) = self.inbox.try_recv() {
            self.link.receive(packet, time);
        }
        if let Some(packet) = self.link.outgoing(time, angle, steps, flags) {
            if self.now.send(self.peer, &packet.encode()).is_ok() {
                self.sent = self.sent.wrapping_add(1);
            } else {
                self.errors = self.errors.wrapping_add(1);
            }
        }
    }
}
