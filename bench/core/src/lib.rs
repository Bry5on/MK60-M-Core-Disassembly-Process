//! e46-mk60-tool-core - reusable E46 MK60 Tool logic, shared by the CLI (`host`) and the GUI.
//!
//! All transport and protocol lives here: the Gauge.S serial bridge, CCP (upload/dump),
//! KWP (seed/key arm over K-line), and CRO/DTO discovery. No CLI parsing or UI - callers
//! supply their own front end. OS-agnostic; the only platform surface is the serial port
//! (see [`bridge`]).

pub mod bridge;
pub mod ftdi;
pub mod kwp;
pub mod mk60can;
pub mod ops;
pub mod proto;
