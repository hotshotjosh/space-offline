//! Operating-system randomness: the one file that imports `getrandom`.

use std::io;

/// Fill `buf` with bytes from the operating system's CSPRNG.
///
/// An empty buffer is fine and does nothing. The error carries the OS error
/// code when there is one.
pub fn fill(buf: &mut [u8]) -> io::Result<()> {
    getrandom::getrandom(buf).map_err(|e| match e.raw_os_error() {
        Some(code) => io::Error::from_raw_os_error(code),
        None => io::Error::other(format!("os random: {e}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_fills_differ() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        fill(&mut a).unwrap();
        fill(&mut b).unwrap();
        assert_ne!(a, b);
        assert_ne!(a, [0u8; 32]);
    }

    #[test]
    fn a_zero_length_fill_is_ok() {
        fill(&mut []).unwrap();
    }
}
