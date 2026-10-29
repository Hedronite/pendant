use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

use crate::error::HerdrError;

/// Cap for one NDJSON or observe line from herdr.
pub const MAX_HERDR_LINE: usize = 1024 * 1024;

/// Read one line, excluding the trailing newline.
///
/// A line whose body is longer than `limit` is an error. The read stops at
/// `limit + 1` bytes so a huge line is not buffered.
pub async fn read_limited_line<R>(reader: &mut R, limit: usize) -> Result<String, HerdrError>
where
    R: AsyncBufRead + Unpin,
{
    let mut buf = Vec::new();
    let n = (&mut *reader)
        .take((limit as u64).saturating_add(1))
        .read_until(b'\n', &mut buf)
        .await?;
    if n == 0 {
        return Err(HerdrError::EmptyResponse);
    }
    let newline = buf.last().copied() == Some(b'\n');
    let body_len = if newline { buf.len() - 1 } else { buf.len() };
    if body_len > limit {
        return Err(HerdrError::LineTooLong { limit });
    }
    if newline {
        buf.pop();
        if buf.last().copied() == Some(b'\r') {
            buf.pop();
        }
    }
    String::from_utf8(buf).map_err(|_| HerdrError::InvalidLine)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    use tokio::io::BufReader;

    #[tokio::test]
    async fn line_longer_than_the_cap_is_an_error() {
        let mut reader = BufReader::new(Cursor::new(&b"0123456789\n"[..]));
        let err = read_limited_line(&mut reader, 4).await.unwrap_err();
        assert!(matches!(err, HerdrError::LineTooLong { limit: 4 }));
    }

    #[tokio::test]
    async fn line_within_the_cap_is_returned() {
        let mut reader = BufReader::new(Cursor::new(&b"ab\n"[..]));
        let line = read_limited_line(&mut reader, 4).await.unwrap();
        assert_eq!(line, "ab");
    }
}
