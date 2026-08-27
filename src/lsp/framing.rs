//! LSP の base protocol のフレーミング。
//!
//! JSON-RPC の payload そのものには長さが入っていないので、`Content-Length` ヘッダで
//! 区切りを付けて 1 本のストリームに並べる。ここが持つのはその区切りだけで、
//! 中身が JSON として何であるかは [`super::message`] の担当。
//!
//! **読み取りを `BufRead` で受けるのは、サーバ無しで確かめられる形にするため**
//! (rules/tdd.md「`lsp` は『応答を受け取ってから先』を切り出す」)。

use std::error::Error;
use std::fmt;
use std::io::{self, BufRead};

/// 長さを載せるヘッダの名前。比較は ASCII 大文字小文字を無視して行う。
const CONTENT_LENGTH: &str = "content-length";

/// payload を 1 フレームのバイト列にする。
///
/// 長さは**文字数ではなくバイト数**。ヘッダが数えているのは受け手が読み取るバイト数で、
/// 非 ASCII を含む payload では文字数と食い違う。
pub fn framed_bytes_of(payload: &str) -> Vec<u8> {
    let mut framed = format!("Content-Length: {}\r\n\r\n", payload.len()).into_bytes();
    framed.extend_from_slice(payload.as_bytes());
    framed
}

/// 次の 1 フレームを読み、その payload を返す。
///
/// `reader` は読み進める位置を保つので、続けて呼べば次のフレームが返る。
///
/// # Errors
///
/// フレームの切れ目で入力が尽きた（サーバが終了した）とき、フレームの途中で尽きたとき、
/// `Content-Length` が無い / 数として読めないとき、payload が UTF-8 でないとき、
/// 読み取り自体が失敗したとき。
pub fn payload_of<R: BufRead>(reader: &mut R) -> Result<String, FramingError> {
    let length = content_length_of(reader)?;

    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            return FramingError::TruncatedFrame;
        }
        FramingError::Read(error)
    })?;

    String::from_utf8(payload).map_err(|_| FramingError::NotUtf8)
}

/// ヘッダ部を空行まで読み進め、`Content-Length` の値を返す。
///
/// # Errors
///
/// ヘッダが 1 行も無いまま入力が尽きたとき（サーバの終了）、空行に届く前に尽きたとき、
/// `Content-Length` が無い / 数として読めないとき。
fn content_length_of<R: BufRead>(reader: &mut R) -> Result<usize, FramingError> {
    let mut length = None;
    let mut header_started = false;

    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).map_err(FramingError::Read)?;

        if read == 0 {
            // フレームの切れ目での EOF とフレーム途中での EOF を分ける。前者はサーバが
            // 終了しただけで、後者は落ちたか壊れたかなので、呼び出し側が直す先が違う。
            if header_started {
                return Err(FramingError::TruncatedFrame);
            }
            return Err(FramingError::ServerClosed);
        }

        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        header_started = true;

        if let Some(value) = content_length_value_of(line) {
            length = Some(
                value
                    .parse()
                    .map_err(|_| FramingError::MalformedContentLength {
                        value: value.to_owned(),
                    })?,
            );
        }
    }

    length.ok_or(FramingError::MissingContentLength)
}

/// ヘッダ 1 行が `Content-Length` なら、その値の文字列を返す。
///
/// 他のヘッダ（`Content-Type` など）は仕様上並びうるので、`None` で読み飛ばさせる。
fn content_length_value_of(line: &str) -> Option<&str> {
    let (name, value) = line.split_once(':')?;

    if !name.trim().eq_ignore_ascii_case(CONTENT_LENGTH) {
        return None;
    }

    Some(value.trim())
}

/// フレームの読み取りが失敗した理由。
#[derive(Debug)]
pub enum FramingError {
    /// フレームの切れ目で入力が尽きた。サーバが終了している。
    ServerClosed,
    /// フレームの途中で入力が尽きた。
    TruncatedFrame,
    /// ヘッダに `Content-Length` が無い。
    MissingContentLength,
    /// `Content-Length` の値を数として読めない。
    MalformedContentLength {
        /// 読めなかった値そのもの。
        value: String,
    },
    /// payload を UTF-8 として解釈できない。
    NotUtf8,
    /// 読み取り自体が失敗した。
    Read(io::Error),
}

impl fmt::Display for FramingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ServerClosed => write!(formatter, "LSP サーバが終了しています"),
            Self::TruncatedFrame => {
                write!(formatter, "LSP サーバの応答がフレームの途中で切れています")
            }
            Self::MissingContentLength => {
                write!(formatter, "LSP サーバの応答に Content-Length がありません")
            }
            Self::MalformedContentLength { value } => {
                write!(formatter, "Content-Length を数として読めません: {value}")
            }
            Self::NotUtf8 => write!(formatter, "LSP サーバの応答が UTF-8 ではありません"),
            Self::Read(cause) => write!(formatter, "LSP サーバの応答を読めません: {cause}"),
        }
    }
}

impl Error for FramingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ServerClosed
            | Self::TruncatedFrame
            | Self::MissingContentLength
            | Self::MalformedContentLength { .. }
            | Self::NotUtf8 => None,
            Self::Read(cause) => Some(cause),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_payload(framed: &str) -> Result<String, FramingError> {
        payload_of(&mut framed.as_bytes())
    }

    #[test]
    fn test_framed_bytes_of_a_payload_counts_bytes_not_characters() {
        // 9 文字だが 11 バイト。文字数を数える実装ではここで 9 が出る
        let framed = framed_bytes_of(r#"{"a":"あ"}"#);

        assert_eq!(
            String::from_utf8(framed).expect("組み立てた結果は UTF-8"),
            "Content-Length: 11\r\n\r\n{\"a\":\"あ\"}"
        );
    }

    #[test]
    fn test_payload_of_a_framed_message_returns_the_body() {
        let payload = read_payload("Content-Length: 7\r\n\r\n{\"a\":1}").expect("読める");

        assert_eq!(payload, r#"{"a":1}"#);
    }

    #[test]
    fn test_payload_of_skips_headers_other_than_content_length() {
        let framed =
            "Content-Length: 7\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{\"a\":1}";

        assert_eq!(read_payload(framed).expect("読める"), r#"{"a":1}"#);
    }

    #[test]
    fn test_payload_of_accepts_a_content_length_header_in_any_case() {
        let payload = read_payload("content-length: 7\r\n\r\n{\"a\":1}").expect("読める");

        assert_eq!(payload, r#"{"a":1}"#);
    }

    #[test]
    fn test_payload_of_called_twice_returns_the_second_frame() {
        // 1 フレーム目で読み過ぎていると、ここで 2 つ目の本文に届かない
        let framed = "Content-Length: 7\r\n\r\n{\"a\":1}Content-Length: 7\r\n\r\n{\"b\":2}";
        let mut reader = framed.as_bytes();

        payload_of(&mut reader).expect("1 フレーム目は読める");

        assert_eq!(
            payload_of(&mut reader).expect("2 フレーム目も読める"),
            r#"{"b":2}"#
        );
    }

    #[test]
    fn test_payload_of_at_the_end_of_input_reports_the_server_closed() {
        let error = read_payload("").expect_err("読めない");

        assert!(matches!(error, FramingError::ServerClosed));
    }

    #[test]
    fn test_payload_of_with_headers_cut_off_reports_a_truncated_frame() {
        // ヘッダを読み始めた後の EOF は、サーバの正常な終了とは別物
        let error = read_payload("Content-Length: 7\r\n").expect_err("読めない");

        assert!(matches!(error, FramingError::TruncatedFrame));
    }

    #[test]
    fn test_payload_of_with_a_body_shorter_than_content_length_reports_a_truncated_frame() {
        let error = read_payload("Content-Length: 7\r\n\r\n{\"a\"").expect_err("読めない");

        assert!(matches!(error, FramingError::TruncatedFrame));
    }

    #[test]
    fn test_payload_of_without_content_length_reports_it_missing() {
        let framed = "Content-Type: application/vscode-jsonrpc\r\n\r\n{\"a\":1}";

        let error = read_payload(framed).expect_err("読めない");

        assert!(matches!(error, FramingError::MissingContentLength));
    }

    #[test]
    fn test_payload_of_with_a_non_numeric_content_length_reports_it_malformed() {
        let error = read_payload("Content-Length: seven\r\n\r\n{\"a\":1}").expect_err("読めない");

        assert!(matches!(
            error,
            FramingError::MalformedContentLength { value } if value == "seven"
        ));
    }

    #[test]
    fn test_payload_of_with_a_non_utf8_body_reports_it() {
        let mut framed = b"Content-Length: 2\r\n\r\n".to_vec();
        framed.extend_from_slice(&[0xff, 0xfe]);

        let error = payload_of(&mut framed.as_slice()).expect_err("読めない");

        assert!(matches!(error, FramingError::NotUtf8));
    }
}
