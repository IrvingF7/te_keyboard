#include "class/cdc/cdc.h"

// esp-idf-sys 0.36's bindgen produces an aligned nested bitfield inside this
// packed, unused telephony descriptor, which Rust rejects (E0588). Preserve its
// exact size/alignment as opaque bytes. No USB runtime code uses this type.
// https://rust-lang.github.io/rust-bindgen/replacing-types.html
/** <div rustbindgen replaces="cdc_desc_func_telephone_call_state_reporting_capabilities_t"></div> */
struct te_keyboard_cdc_capabilities_storage {
    unsigned char bytes[sizeof(cdc_desc_func_telephone_call_state_reporting_capabilities_t)];
};
_Static_assert(_Alignof(cdc_desc_func_telephone_call_state_reporting_capabilities_t) == 1,
               "Expected a packed CDC telephony descriptor");
