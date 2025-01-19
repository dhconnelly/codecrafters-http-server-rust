use std::{error::Error, fmt::Display};

use crate::Request;

#[derive(Debug)]
pub struct DecompressionError;

impl Display for DecompressionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to decompress")
    }
}

impl Error for DecompressionError {}

pub fn decompress(req: &mut Request) -> Result<(), DecompressionError> {
    Ok(())
}
