#include <Arduino.h>
#include "encoder.h"
#include "esp_now_link.h"

// This file is the knob's application. Arduino calls setup() once after boot,
// then repeatedly calls loop(). Each pass reads hardware, services the radio,
// and logs a little information; no PC program needs to drive this loop.
// An unnamed namespace keeps these names local to this .cpp file. Variables
// here have static storage duration, so their values persist between calls.
namespace {
Encoder encoder;
link_test::Link radio;
constexpr int kBootButton = 9;
constexpr int kMotorPins[] = {6, 5, 1, 3, 0, 4};
// The encoder has 14 bits of angle: 2^14 = 16384 counts per full turn.
constexpr float kDegreesPerCount = 360.0f / 16384;
constexpr float kStepDegrees = 15.0f;  // 24 logical steps per revolution.
uint16_t angle = 0;
uint16_t previousAngle = 0;
bool haveAngle = false;
// angle wraps once per turn; position accumulates motion across many turns.
// int64_t provides more room for that running total than the raw 14-bit angle.
int64_t position = 0;
int32_t steps = 0;
// Keep fractional movement left over after producing a whole 15-degree step.
float stepRemainder = 0;
uint32_t goodReads = 0;
uint32_t badReads = 0;
uint32_t lastGoodRead = 0;
uint32_t lastPrint = 0;
int64_t printedPosition = 0;
// raw is the latest electrical reading; stable is the debounced button state.
int buttonRaw = HIGH;
int buttonStable = HIGH;
uint32_t buttonChanged = 0;
uint32_t lastLinkPrint = 0;
bool lastConnected = false;
}

void setup() {
  // All six driver inputs LOW: motor unpowered/free to turn.
  // GPIO numbers are MCU pins. Set their direction before driving a value;
  // configure the motor before starting any other peripheral in this test.
  for (int pin : kMotorPins) {
    pinMode(pin, OUTPUT);
    digitalWrite(pin, LOW);
  }
  // A pull-up holds the input HIGH when released. Pressing BOOT connects it to
  // ground (LOW), so the button is "active low". This is the PCB button, not a
  // switch inside the rotating shaft.
  pinMode(kBootButton, INPUT_PULLUP);
  Serial.begin(115200);
  // On this board Serial is USB CDC (configured in platformio.ini). Give a
  // serial monitor up to 2 s to attach, then continue even if USB is power-only.
  const uint32_t start = millis();
  while (!Serial && millis() - start < 2000) delay(10);
  Serial.println("\nT-Knob ESP-NOW + encoder test; motor disabled.");
  Serial.println("Turn the knob; press BOOT to test the board button.");
  encoder.begin();
  if (!radio.begin(link_test::Role::Knob)) Serial.println("Radio setup failed.");
}

void loop() {
  // Sample time once so the timers in this pass use a common reference.
  const uint32_t now = millis();
  uint16_t sample;
  if (encoder.read(sample)) {
    ++goodReads;
    lastGoodRead = now;
    angle = sample;
    if (haveAngle) {
      // The first sample sets the starting point; it is not movement from zero.
      int delta = int(angle) - int(previousAngle);
      // Unwrap the 0/360-degree boundary before accumulating movement.
      // Example: 16380 -> 4 is +8 counts, not -16376. Choose the shorter
      // direction, assuming less than half a turn between successful samples.
      if (delta > 8192) delta -= 16384;
      if (delta < -8192) delta += 16384;
      position += delta;
      stepRemainder += delta * kDegreesPerCount;
      // One read may cross multiple logical steps. Subtract each completed
      // step but preserve the remainder; throwing it away would lose movement.
      while (stepRemainder >= kStepDegrees) {
        ++steps;
        stepRemainder -= kStepDegrees;
      }
      while (stepRemainder <= -kStepDegrees) {
        --steps;
        stepRemainder += kStepDegrees;
      }
    }
    previousAngle = angle;
    haveAngle = true;
  } else {
    // Failed SPI/CRC reads do not replace the last valid angle. A long gap can
    // still make accumulated movement ambiguous because whole turns are unknown.
    ++badReads;
  }

  const int button = digitalRead(kBootButton);
  // Mechanical contacts can bounce rapidly during one press. Restart a timer
  // whenever the electrical level changes, then accept it only after 30 ms
  // without another change. This avoids a blocking delay for button handling.
  if (button != buttonRaw) {
    buttonRaw = button;
    buttonChanged = now;
  }
  if (buttonRaw != buttonStable && now - buttonChanged >= 30) {
    buttonStable = buttonRaw;
    Serial.println(buttonStable == LOW ? "BOOT pressed" : "BOOT released");
  }

  // Report sensor data as valid only if a good read happened in the last 500 ms.
  // | combines the two bit flags. ?: selects a flag or zero for each condition.
  // This sends current button state, so a very short press between heartbeats
  // can be missed; a later application protocol could queue individual events.
  const uint8_t flags =
      (haveAngle && goodReads && now - lastGoodRead < 500 ? link_test::kEncoderValid : 0) |
      (buttonStable == LOW ? link_test::kButtonPressed : 0);
  // tick() processes queued radio messages and sends only when its timer is due.
  // It is called every pass, not just when the knob moves, so an idle knob can
  // still prove that it is connected.
  radio.tick(now, angle, steps, flags);
  const bool connected = radio.connected(now);
  // Print immediately on a connection change, otherwise once per second.
  if (connected != lastConnected || now - lastLinkPrint >= 1000) {
    Serial.printf("LINK %s tx=%lu rx=%lu errors=%lu\n", radio.status(now),
                  (unsigned long)radio.sent(), (unsigned long)radio.received(),
                  (unsigned long)radio.sendErrors());
    lastConnected = connected;
    lastLinkPrint = now;
  }

  // Limit movement logs to 10 Hz, but always print a status line each second
  // at rest. Unthrottled logging can dominate a fast embedded loop.
  if ((now - lastPrint >= 100 && position != printedPosition) ||
      now - lastPrint >= 1000) {
    if (haveAngle) {
      Serial.printf("angle=%.2f deg  relative=%.2f deg  steps=%ld  ok=%lu  errors=%lu\n",
                    angle * kDegreesPerCount, double(position) * kDegreesPerCount,
                    long(steps), (unsigned long)goodReads, (unsigned long)badReads);
    } else {
      Serial.printf("Waiting for valid MT6701 data; errors=%lu\n",
                    (unsigned long)badReads);
    }
    lastPrint = now;
    printedPosition = position;
  }
  // Yield briefly to the scheduler instead of spinning at full CPU speed.
  // This is an encoder/radio bench test, not a high-frequency motor-control loop.
  delay(5);
}
