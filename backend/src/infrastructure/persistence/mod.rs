//src/infrastructure/persistence/mod.rs
pub mod elasticsearch;
pub mod manticore;
pub mod manticore_high;
mod manticore_http;
pub mod mongodb;
pub mod ports;
pub mod redis;
pub mod scylla;
pub(crate) mod stack;
