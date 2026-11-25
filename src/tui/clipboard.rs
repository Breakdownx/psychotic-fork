//! Report copying to the local clipboard or the terminal over OSC 52.

use std::io::Write as _;

pub(super) enum Copied {
    Clipboard,
    Terminal,
}

/// Windows has a clipboard of its own to put the text on. Anywhere else this
/// may be a server with none, so the text goes to the terminal as OSC 52, which
/// a terminal that supports it copies on the machine the user sits at.
pub(super) fn copy_out(text: &str) -> Copied {
    #[cfg(windows)]
    if crate::utils::set_clipboard_text(text) {
        return Copied::Clipboard;
    }
    let _ = copy_to_terminal(text);
    Copied::Terminal
}

fn copy_to_terminal(text: &str) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    out.flush()
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64("ключ".as_bytes()), "0LrQu9GO0Yc=");
    }
}
