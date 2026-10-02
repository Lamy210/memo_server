//src/infrastructure/persistence/mod.rs
pub mod elasticsearch;
pub mod manticore;
mod manticore_http;
pub mod manticore_high;
pub mod mongodb;
pub mod ports;
pub mod redis;
pub mod scylla;
pub(crate) mod stack;
