//! Portable scene simulation. Native-reference equations and portable policy are
//! distinguished in each subsystem; this crate performs no OS or filesystem IO.
#![forbid(unsafe_code)]

pub mod math;
pub mod model;
pub mod noise;
pub mod particle;
pub mod rng;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

pub mod children;
