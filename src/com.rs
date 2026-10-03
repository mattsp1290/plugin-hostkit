//! VST3 COM types, vtable definitions, and host-side COM object implementations.
//!
//! These types mirror the VST3 SDK C++ structs with `#[repr(C)]` to ensure
//! binary-compatible layouts for FFI calls.

use std::collections::HashMap;
use std::os::raw::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

mod abi;
mod connection;
mod diagnostics;
mod host;
mod message;
mod processing;
mod stream;

pub(crate) use abi::*;
pub(crate) use connection::*;
pub(crate) use diagnostics::*;
pub(crate) use host::*;
use message::{AttributeListObj, IID_IATTRIBUTE_LIST, IID_IMESSAGE, MessageObj};
pub(crate) use processing::*;
pub(crate) use stream::*;
