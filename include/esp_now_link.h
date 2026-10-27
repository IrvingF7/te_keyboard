#pragma once

// Shared by two separate firmware builds, one running on each board.
// Each MCU has its own Link object and memory; only Packet bytes cross the radio.
// Read begin() for startup, tick() for normal operation, and receive() for the
// handoff from the background Wi-Fi task to the Arduino loop().
// #pragma once prevents duplicate definitions when a header is included twice.

#include <Arduino.h>
#include <WiFi.h>
#include <esp_idf_version.h>
#include <esp_now.h>
#include <esp_system.h>
#include <esp_wifi.h>
#include <freertos/FreeRTOS.h>
#include <freertos/queue.h>
#include <cstring>

namespace link_test {
// constexpr values are fixed at compile time. Both radios must use the same
// channel, like two walkie-talkies tuned to the same frequency channel.
constexpr uint8_t kChannel = 1;
constexpr uint32_t kIntervalMs = 250;
constexpr uint32_t kTimeoutMs = 2000;
// MAC addresses identify the destination radio. These belong to our actual
// boards, so replacing a board requires updating its address here.
constexpr uint8_t kKnobMac[] = {0x40, 0x4c, 0xca, 0x5c, 0x39, 0x30};
constexpr uint8_t kDisplayMac[] = {0x80, 0xb5, 0x4e, 0xc7, 0xf3, 0xdc};
// Bit flags pack independent yes/no values into one byte: 1 is binary 00000001
// and 2 is 00000010. Use | to combine flags and & to test an individual flag.
constexpr uint8_t kEncoderValid = 1;
constexpr uint8_t kButtonPressed = 2;
enum class Role : uint8_t { Knob = 1, Display = 2 };

// Both targets are little-endian ESP32s. Keep the wire layout fixed and small
// enough for ESP-NOW v1 (the display uses an older Arduino/ESP-IDF stack).
// uint8_t/uint16_t/uint32_t have exactly 8/16/32 bits; int32_t also allows
// negative values, which we need for signed rotation steps. A struct groups
// these fields into one message. We send its raw bytes, so both builds must
// agree on field order, sizes, alignment, and byte order. For other platforms,
// explicitly encoding each field would be more portable than copying a struct.
struct Packet {
  uint32_t magic = 0x544b4c31;  // T-Keyboard link test, protocol v1.
  uint8_t version = 1;
  Role role = Role::Knob;
  uint8_t flags = 0;  // Encoder validity and current button state.
  // Reserved fields are initialized to zero and leave space for future changes.
  uint8_t reserved = 0;
  uint32_t session = 0;   // Random ID for this boot; changes after a restart.
  uint32_t sequence = 0;  // Increasing heartbeat number within this boot.
  // Echo means "I received your packet with this session and sequence."
  uint32_t echoSession = 0;
  uint32_t echoSequence = 0;
  int32_t steps = 0;    // Cumulative logical steps, not just the latest change.
  uint16_t angle = 0;   // Raw encoder count, 0..16383 for one revolution.
  uint16_t reserved2 = 0;
};
// This check runs at compile time, not on the MCU. It catches size changes;
// keep the protocol version/layout in sync too, since equal size alone is not
// enough to guarantee compatible messages.
static_assert(sizeof(Packet) == 32, "ESP-NOW wire layout changed");

// Link owns radio setup, heartbeat timing, and the last peer reading. Members
// ending in _ are stored state; they survive from one tick() call to the next.
class Link {
 public:
  // Called once from setup(). A false result leaves ready_ false, so tick()
  // safely does nothing and the application can show a radio error.
  bool begin(Role role) {
    role_ = role;
    memcpy(peerMac_, role == Role::Knob ? kDisplayMac : kKnobMac, 6);
    // FreeRTOS is the ESP32's task scheduler. Its queue is a thread-safe mailbox
    // between the Wi-Fi task and our loop. It stores copies of up to 8 packets.
    queue_ = xQueueCreate(8, sizeof(Packet));
    if (!queue_) return false;
    instance() = this;
    // Zero means "no session yet" in outgoing echoes, so do not use it as an ID.
    do { session_ = esp_random(); } while (!session_);
    // Use the Wi-Fi radio without joining a router. Avoid saving configuration
    // to flash, disconnect any AP connection, and disable radio power-saving
    // delays for this always-powered bench test. ESP-NOW is not a TCP/IP link.
    WiFi.persistent(false);
    if (!WiFi.mode(WIFI_STA)) return false;
    WiFi.disconnect(false, false);
    if (!WiFi.setSleep(false)) return false;
    if (!check(esp_wifi_set_channel(kChannel, WIFI_SECOND_CHAN_NONE))) return false;
    Serial.printf("ESP-NOW local=%s peer=%02X:%02X:%02X:%02X:%02X:%02X channel=%u\n",
                  WiFi.macAddress().c_str(), peerMac_[0], peerMac_[1], peerMac_[2],
                  peerMac_[3], peerMac_[4], peerMac_[5], kChannel);
    if (!check(esp_now_init())) return false;
    // {} zero-initializes every field before we fill in the ones we need.
    // A peer entry tells ESP-NOW where and how to send outgoing packets.
    esp_now_peer_info_t peer = {};
    memcpy(peer.peer_addr, peerMac_, 6);
    peer.channel = kChannel;
    peer.ifidx = WIFI_IF_STA;
    peer.encrypt = false;  // Simple bench test, fixed peer, no pairing UI.
    if (!check(esp_now_add_peer(&peer))) return false;
    // Register a function for ESP-NOW to call whenever a packet arrives.
    if (!check(esp_now_register_recv_cb(onReceive))) return false;
    ready_ = true;
    return true;
  }

  void tick(uint32_t now, uint16_t angle = 0, int32_t steps = 0,
            uint8_t flags = 0) {
    // Called repeatedly by loop(), never waits for a reply. The display uses
    // the default telemetry arguments; only the knob sends encoder/button data.
    if (!ready_) return;
    Packet packet;
    // The final argument 0 means "do not block if the queue is empty".
    // Drain all available packets before deciding whether to send a heartbeat.
    while (xQueueReceive(queue_, &packet, 0) == pdTRUE) {
      // Radio packets can be repeated or arrive late. Within the same boot,
      // ignore packets older than the most recent one already processed.
      if (haveRemote_ && packet.session == remote_.session &&
          packet.sequence <= remote_.sequence) continue;
      if (haveRemote_ && packet.session != remote_.session) {
        // A new peer boot needs a new round trip before showing CONNECTED.
        haveRoundTrip_ = false;
        lastEcho_ = 0;
      }
      remote_ = packet;
      haveRemote_ = true;
      lastReceive_ = now;
      ++received_;
      // A fresh echo of this boot's recently sent heartbeat proves that the
      // other MCU received us. Seeing only incoming packets is insufficient.
      // Example: knob sends #12, bar replies "I saw knob #12". The knob now
      // knows both directions work. Both boards do this independently.
      // Accept only echoes of our current boot, no more than 8 sends old
      // (about 2 s), and newer than the last echo counted as confirmation.
      if (packet.echoSession == session_ && packet.echoSequence != 0 &&
          packet.echoSequence <= sequence_ &&
          sequence_ - packet.echoSequence <= 8 &&
          packet.echoSequence > lastEcho_) {
        lastEcho_ = packet.echoSequence;
        lastRoundTrip_ = now;
        haveRoundTrip_ = true;
      }
    }
    // millis() is an unsigned counter that wraps after about 50 days. Comparing
    // elapsed time as now - previous keeps these short timers working across
    // that wrap. Do not use delay(250): the loop has other work between sends.
    if (now - lastSend_ < kIntervalMs) return;
    lastSend_ = now;
    Packet outgoing;
    outgoing.role = role_;
    outgoing.session = session_;
    outgoing.sequence = ++sequence_;  // Increment first, then use the new value.
    // Piggyback the acknowledgement on the next heartbeat instead of sending
    // a separate ACK packet. Until we hear from the peer, echo fields stay 0.
    outgoing.echoSession = haveRemote_ ? remote_.session : 0;
    outgoing.echoSequence = haveRemote_ ? remote_.sequence : 0;
    outgoing.angle = angle;
    outgoing.steps = steps;
    outgoing.flags = flags;
    // TX counts API submissions; CONNECTED depends on application-level echoes.
    // reinterpret_cast exposes the message as bytes; it does not encode it.
    // ESP_OK means the send request was accepted, not that the peer's loop
    // processed it. That distinction is why we check echoed packet numbers.
    if (esp_now_send(peerMac_, reinterpret_cast<const uint8_t *>(&outgoing),
                     sizeof(outgoing)) == ESP_OK) ++sent_;
    else ++sendErrors_;
  }

  bool ready() const { return ready_; }
  bool connected(uint32_t now) const {
    // const means this query does not modify the Link object's stored state.
    // Require both recent incoming traffic and recent round-trip confirmation:
    // one radio direction can fail while the other continues receiving.
    return haveRemote_ && haveRoundTrip_ &&
           now - lastReceive_ < kTimeoutMs && now - lastRoundTrip_ < kTimeoutMs;
  }
  bool hasRemote() const { return haveRemote_; }
  // A const reference lets the UI inspect the stored packet without copying it
  // or changing it. Only tick() replaces it, in the same task as the UI.
  const Packet &remote() const { return remote_; }
  uint32_t received() const { return received_; }
  uint32_t sent() const { return sent_; }
  uint32_t sendErrors() const { return sendErrors_; }
  uint32_t age(uint32_t now) const { return now - lastReceive_; }
  const char *status(uint32_t now) const {
    if (!ready_) return "RADIO ERROR";
    if (connected(now)) return "CONNECTED";
    return haveRemote_ ? "DISCONNECTED" : "WAITING FOR PEER";
  }

 private:
  // The C-style callback API cannot call an ordinary object's member function
  // directly. Keep one pointer to this firmware's Link so a static callback can
  // find it. This helper assumes one Link object per MCU, initialized once.
  static Link *&instance() { static Link *value = nullptr; return value; }
  static bool check(esp_err_t error) {
    if (error == ESP_OK) return true;
    Serial.printf("ESP-NOW init error: %s\n", esp_err_to_name(error));
    return false;
  }
  static void receive(const uint8_t *mac, const uint8_t *data, int length) {
    Link *self = instance();
    // Check the sender and byte count before interpreting the payload. The MAC
    // filter prevents accidental cross-talk; it is not authentication/encryption.
    if (!self || !mac || !data || length != sizeof(Packet) ||
        memcmp(mac, self->peerMac_, 6) != 0) return;
    Packet packet;
    // The callback's data pointer is borrowed and must not be saved for later.
    // Copy into a properly aligned local Packet, then validate its fields.
    memcpy(&packet, data, sizeof(packet));
    const Role expected = self->role_ == Role::Knob ? Role::Display : Role::Knob;
    if (packet.magic != 0x544b4c31 || packet.version != 1 ||
        packet.role != expected || !packet.session || !packet.sequence) return;
    // Wi-Fi task callback: copy only. Radio sends, logging and TFT work happen
    // in loop(). Dropping a heartbeat when the queue is full is harmless.
    // This callback runs in a normal RTOS task, not an interrupt service routine
    // (ISR), so use xQueueSend rather than the special xQueueSendFromISR API.
    // The zero timeout prevents waiting for space. Repeated heartbeats carry
    // fresh state; this queue policy would need reconsidering for unique events.
    xQueueSend(self->queue_, &packet, 0);
  }
// ESP-IDF is Espressif's underlying SDK. The C6 and S3 builds use different SDK
// versions whose callbacks have different parameter types. The preprocessor
// selects one adapter at compile time; both call the same receive() logic.
#if ESP_IDF_VERSION_MAJOR >= 5
  static void onReceive(const esp_now_recv_info_t *info, const uint8_t *data,
                        int length) {
    if (info) receive(info->src_addr, data, length);
  }
#else
  static void onReceive(const uint8_t *mac, const uint8_t *data, int length) {
    receive(mac, data, length);
  }
#endif
  // Radio identity and the mailbox used by the receive callback.
  Role role_ = Role::Knob;
  uint8_t peerMac_[6] = {};
  QueueHandle_t queue_ = nullptr;
  // After begin(), loop()/tick() own the state below; the callback only queues
  // received packets rather than modifying this connection/telemetry state.
  bool ready_ = false;
  bool haveRemote_ = false;
  bool haveRoundTrip_ = false;
  Packet remote_;
  // Our outgoing message numbers and the most recent echo of one of them.
  uint32_t session_ = 0;
  uint32_t sequence_ = 0;
  uint32_t lastEcho_ = 0;
  // All timestamps use the local MCU's millis(); board clocks need not match.
  uint32_t lastSend_ = 0;
  uint32_t lastReceive_ = 0;
  uint32_t lastRoundTrip_ = 0;
  // Diagnostic counters. sendErrors_ counts rejected send requests, not every
  // packet lost in the air; missing echoes are what reveal a broken link.
  uint32_t received_ = 0;
  uint32_t sent_ = 0;
  uint32_t sendErrors_ = 0;
};
}  // namespace link_test
