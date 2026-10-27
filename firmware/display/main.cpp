#include <Arduino.h>
#include <TFT_eSPI.h>
#include "esp_now_link.h"

// The bar has its own setup()/loop(), independent of the knob. It receives
// encoder telemetry over ESP-NOW and draws it; it does not read the knob's SPI
// sensor directly. Global objects persist for the lifetime of the firmware.
TFT_eSPI display;
link_test::Link radio;

void drawStatus(uint32_t now) {
  // A fixed buffer avoids constructing variable-length Strings in every redraw.
  // snprintf formats text and respects the buffer's size, including its final
  // null terminator. drawString uses pixel x/y coordinates and font number 2.
  char text[64];
  display.setTextColor(TFT_CYAN, TFT_BLACK);
  snprintf(text, sizeof(text), "ESP-NOW CH1  TX:%lu RX:%lu",
           (unsigned long)radio.sent(), (unsigned long)radio.received());
  display.drawString(text, 4, 0, 2);
  display.setTextColor(radio.connected(now) ? TFT_GREEN : TFT_YELLOW, TFT_BLACK);
  display.drawString(radio.status(now), 4, 19, 2);
  display.setTextColor(TFT_WHITE, TFT_BLACK);
  if (radio.hasRemote()) {
    // auto lets C++ infer Packet; const & reads the existing packet without
    // copying or modifying it. hasRemote() means we have ever received a packet,
    // not necessarily that it is fresh: disconnected values remain on screen.
    const auto &packet = radio.remote();
    // Bitwise & tests whether the sender marked this angle as valid.
    if (packet.flags & link_test::kEncoderValid) {
      snprintf(text, sizeof(text), "Angle:%.1f  Steps:%ld",
               packet.angle * (360.0 / 16384), (long)packet.steps);
    } else {
      snprintf(text, sizeof(text), "Encoder: waiting for valid data");
    }
    display.drawString(text, 4, 38, 2);
    snprintf(text, sizeof(text), "BOOT:%s  Last RX:%lu ms",
             packet.flags & link_test::kButtonPressed ? "DOWN" : "UP",
             (unsigned long)radio.age(now));
  } else {
    display.drawString("Power on the knob to connect", 4, 38, 2);
    snprintf(text, sizeof(text), "No router needed / motor off");
  }
  display.drawString(text, 4, 57, 2);
}

void setup() {
  Serial.begin(115200);
  // Initialize the screen, then orient its narrow panel horizontally.
  display.init();
  display.setRotation(1);  // 284 x 76 landscape.
  // Draw each line with a full-width background. If "Steps:100" becomes
  // "Steps:9", padding erases the old trailing digits without a full-screen clear.
  display.setTextPadding(display.width() - 4);
  pinMode(TFT_BL, OUTPUT);
  digitalWrite(TFT_BL, TFT_BACKLIGHT_ON);  // Bar backlight is active LOW.
  // These pin/value macros come from the vendor's TFT_eSPI board configuration.
  // LOW turns this board's backlight on; HIGH/LOW meaning depends on the circuit.
  display.fillScreen(TFT_BLACK);
  Serial.println("\nT-Display-Bar ESP-NOW link test");
  if (!radio.begin(link_test::Role::Display)) Serial.println("Radio setup failed.");
  drawStatus(millis());
}

void loop() {
  // Function-local static variables retain their values across loop() calls.
  static uint32_t lastUpdate = 0;
  static uint32_t lastPrint = 0;
  static bool lastConnected = false;
  const uint32_t now = millis();
  // Service the radio every pass, but redraw only four times a second. Radio,
  // display, and logging have separate schedules, so a slow UI rate does not
  // require a long delay that would stop us from processing received packets.
  radio.tick(now);
  if (now - lastUpdate >= 250) {
    lastUpdate = now;
    // Pad all four lines to erase old values without blanking the whole screen.
    drawStatus(now);
  }
  // Read the local connection state rather than trusting a peer's claim that
  // it is connected. Link requires a recent round trip through the actual radio.
  const bool connected = radio.connected(now);
  if (connected != lastConnected || now - lastPrint >= 1000) {
    Serial.printf("LINK %s tx=%lu rx=%lu errors=%lu age_ms=%lu angle=%u steps=%ld boot=%u\n",
                  radio.status(now), (unsigned long)radio.sent(),
                  (unsigned long)radio.received(), (unsigned long)radio.sendErrors(),
                  radio.hasRemote() ? (unsigned long)radio.age(now) : 0UL,
                  radio.remote().angle, (long)radio.remote().steps,
                  !!(radio.remote().flags & link_test::kButtonPressed));
    lastConnected = connected;
    lastPrint = now;
  }
  // Yield briefly so background tasks can run, then service the link again.
  delay(10);
}
