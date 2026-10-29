//! USB wire descriptors for a CDC serial port plus Consumer Control HID.
//! These are byte arrays so their layout is independent of Rust struct layout.

/// One 16-bit Consumer-page usage per input report; zero releases the key.
pub const HID_REPORT_DESCRIPTOR: &[u8] = &[
    0x05, 0x0c, // Usage Page (Consumer)
    0x09, 0x01, // Usage (Consumer Control)
    0xa1, 0x01, // Collection (Application)
    0x15, 0x00, // Logical Minimum (0)
    0x26, 0xff, 0x03, // Logical Maximum (1023)
    0x19, 0x00, // Usage Minimum (0)
    0x2a, 0xff, 0x03, // Usage Maximum (1023)
    0x75, 0x10, // Report Size (16 bits)
    0x95, 0x01, // Report Count (1)
    0x81, 0x00, // Input (Data, Array, Absolute)
    0xc0, // End Collection
];

/// Interfaces 0/1 are CDC; interface 2 is HID. CDC uses endpoints 81/02/82,
/// HID uses 83. The device is bus-powered and does not request remote wake.
pub const CONFIGURATION_DESCRIPTOR: &[u8] = &[
    9,
    2,
    100,
    0,
    3,
    1,
    0,
    0x80,
    250, // Configuration: 100 bytes, 3 interfaces, 500 mA
    8,
    11,
    0,
    2,
    2,
    2,
    0,
    4, // CDC Interface Association Descriptor
    9,
    4,
    0,
    0,
    1,
    2,
    2,
    0,
    4, // CDC control interface
    5,
    0x24,
    0,
    0x20,
    0x01, // CDC header (1.20)
    5,
    0x24,
    1,
    0,
    1, // Call management: data interface 1
    4,
    0x24,
    2,
    2, // ACM: line coding/control supported
    5,
    0x24,
    6,
    0,
    1, // Union: control 0, data 1
    7,
    5,
    0x81,
    3,
    8,
    0,
    16, // CDC notification interrupt IN
    9,
    4,
    1,
    0,
    2,
    0x0a,
    0,
    0,
    0, // CDC data interface
    7,
    5,
    0x02,
    2,
    64,
    0,
    0, // CDC bulk OUT
    7,
    5,
    0x82,
    2,
    64,
    0,
    0, // CDC bulk IN
    9,
    4,
    2,
    0,
    1,
    3,
    0,
    0,
    5, // HID interface (non-boot)
    9,
    0x21,
    0x11,
    0x01,
    0,
    1,
    0x22,
    HID_REPORT_DESCRIPTOR.len() as u8,
    0, // HID 1.11
    7,
    5,
    0x83,
    3,
    2,
    0,
    5, // HID interrupt IN: 2 bytes every 5 ms
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composite_descriptor_has_consistent_lengths_interfaces_and_endpoints() {
        let config = CONFIGURATION_DESCRIPTOR;
        assert_eq!(
            u16::from_le_bytes([config[2], config[3]]) as usize,
            config.len()
        );
        let mut interfaces = Vec::new();
        let mut endpoints = Vec::new();
        let mut offset = 0;
        let mut declared_endpoints = 0;
        while offset < config.len() {
            let len = usize::from(config[offset]);
            assert!(len >= 2 && offset + len <= config.len());
            let d = &config[offset..offset + len];
            match d[1] {
                4 => {
                    interfaces.push((d[2], d[5]));
                    declared_endpoints += usize::from(d[4]);
                }
                5 => {
                    assert!(!endpoints.contains(&d[2]), "duplicate endpoint address");
                    endpoints.push(d[2]);
                    if d[2] == 0x83 {
                        assert_eq!((d[3], d[4], d[5]), (3, 2, 0));
                        assert!(d[6] > 0);
                    }
                }
                0x21 => {
                    assert_eq!(d[6], 0x22);
                    assert_eq!(
                        u16::from_le_bytes([d[7], d[8]]) as usize,
                        HID_REPORT_DESCRIPTOR.len()
                    );
                }
                _ => (),
            }
            offset += len;
        }
        assert_eq!(interfaces.len(), usize::from(config[4]));
        assert_eq!(interfaces, [(0, 2), (1, 0x0a), (2, 3)]);
        assert_eq!(endpoints.len(), declared_endpoints);
        assert_eq!(endpoints, [0x81, 0x02, 0x82, 0x83]);
    }
}
