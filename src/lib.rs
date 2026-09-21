#![cfg_attr(docsrs, feature(doc_cfg))]
//#![warn(missing_docs)]
// #![warn(rustdoc::missing_crate_level_docs)] // Already covered by the extensive example above

//#[cfg(feature = "sys")]
//#[macro_use]
extern crate bitflags;
#[allow(unused_imports)] // Clippy seems to have a false positive with specific feature flags
#[macro_use]
extern crate thiserror;

/// Error types for the netmap library.
pub mod error;
/// Fallback implementations for non-Netmap platforms.
pub mod fallback;
/// Frame structures for representing network packets.
pub mod frame;
/// Netmap interface and builder types.
pub mod netmap;
/// Netmap ring manipulation.
pub mod ring;
//#[cfg(feature = "sys")]

// When `sys` is off, provide no symbols:
//#[cfg(not(feature = "sys"))]
#[allow(missing_docs)]
#[allow(non_upper_case_globals)]
#[allow(non_camel_case_types)]
#[allow(non_snake_case)]
#[allow(unsafe_op_in_unsafe_fn)]
#[allow(unused_unsafe)]
#[allow(clippy::all)]
#[allow(unnecessary_transmutes)]
#[allow(warnings)] 
mod ffi {
    #![allow(non_upper_case_globals)]
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    #![allow(dead_code)]
    include!(concat!(env!("OUT_DIR"), "/binding.rs"));
}

pub use ffi::*;

// Tokio async support (optional feature)
//#[cfg(feature = "tokio-async")]
//#[cfg_attr(docsrs, doc(cfg(feature = "tokio-async")))]
//pub mod tokio_async;
// Re-export async types at the crate root when feature is enabled.
//#[cfg(feature = "tokio-async")]
//#[cfg_attr(docsrs, doc(cfg(feature = "tokio-async")))]
//pub use tokio_async::{AsyncNetmapRxRing, AsyncNetmapTxRing, TokioNetmap};

pub use crate::{error::Error, frame::Frame};

pub mod prelude {
    pub use crate::error::Error;
    pub use crate::frame::Frame;

    //#[cfg(feature = "sys")]
    pub use crate::{
        netmap::{Netmap, NetmapBuilder},
        ring::{Ring, RxRing, TxRing},
    };
}
pub use crate::{
    netmap::{Netmap, NetmapBuilder},
    ring::{Ring, RxRing, TxRing},
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_struct_sizes() {
        //verify that struct sizes match expected values
        assert_eq!(std::mem::size_of::<ffi::netmap_ring>(), 128);
    }
}
