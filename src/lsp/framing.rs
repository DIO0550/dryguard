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
// `Read` は `take` を `&mut R` の側で解決させるために要る。入れないと型引数の側
// （`R: BufRead` の supertrait）で解決し、借りているはずの読み口ごと動かそうとする。
use std::io::{self, BufRead, Read};

/// 長さを載せるヘッダの名前。比較は ASCII 大文字小文字を無視して行う。
const CONTENT_LENGTH: &str = "content-length";

/// 1 フレームの payload として受け取る上限（32 MiB）。
///
/// **`Content-Length` は相手が書いた数**で、こちらはそれを信じてバッファを確保する。
/// 壊れたサーバが桁を間違えるだけで、確保の失敗としてプロセスごと落ちる。
/// 問い合わせるのは hover や references で、応答がこの桁に届くことはない。
const MAX_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;

/// 1 フレームのヘッダ部として読む上限（1 KiB）。
///
/// **payload の上限だけでは足りない。** 長さを知るのはヘッダを読み切った後なので、
/// 改行を送ってこないサーバや、空行に辿り着かないサーバは、[`MAX_PAYLOAD_BYTES`] に
/// 触れないまま読み続けさせられる。行の長さではなく**ヘッダ部の合計**を数えるのは、
/// 短い行を延々と送る形も同じ 1 つの上限で止まるため。
///
/// 実際のヘッダは `Content-Length` と `Content-Type` で 100 バイトに満たない。
const MAX_HEADER_BYTES: usize = 1024;

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
/// `Content-Length` が無い / 数として読めない / 上限を超えるとき、payload が UTF-8 でないとき、
/// 読み取り自体が失敗したとき。
pub fn payload_of<R: BufRead>(reader: &mut R) -> Result<String, FramingError> {
    let length = content_length_of(reader)?;

    // 確保する前に見る。確保してからでは、失敗が `Result` ではなくプロセスの死で返る。
    if length > MAX_PAYLOAD_BYTES {
        return Err(FramingError::PayloadTooLarge { length });
    }

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
/// ヘッダ部が上限を超えたとき、`Content-Length` が無い / 数として読めないとき。
fn content_length_of<R: BufRead>(reader: &mut R) -> Result<usize, FramingError> {
    let mut length = None;
    let mut header_started = false;
    let mut remaining = MAX_HEADER_BYTES;

    loop {
        if remaining == 0 {
            return Err(FramingError::HeaderTooLong);
        }

        // 読める量を残りで縛る。`read_line` は改行が来るまで伸ばし続けるので、
        // 縛らないと 1 行だけで payload の上限を素通りできる。
        let mut line = String::new();
        let mut limited = (&mut *reader).take(remaining as u64);
        let read = limited.read_line(&mut line).map_err(FramingError::Read)?;

        if read == 0 {
            // フレームの切れ目での EOF とフレーム途中での EOF を分ける。前者はサーバが
            // 終了しただけで、後者は落ちたか壊れたかなので、呼び出し側が直す先が違う。
            if header_started {
                return Err(FramingError::TruncatedFrame);
            }
            return Err(FramingError::ServerClosed);
        }

        remaining -= read;

        if !line.ends_with('\n') {
            // 改行が来ないまま止まった。残りが尽きたなら上限、そうでなければ入力が尽きた。
            if remaining == 0 {
                return Err(FramingError::HeaderTooLong);
            }
            return Err(FramingError::TruncatedFrame);
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
    /// `Content-Length` が受け取る上限を超えている。
    PayloadTooLarge {
        /// サーバが申告した長さ。
        length: usize,
    },
    /// ヘッダ部が上限を超えても空行に届かない。
    HeaderTooLong,
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
            Self::PayloadTooLarge { length } => write!(
                formatter,
                "LSP サーバの応答が大きすぎます ({length} バイト / 上限 {MAX_PAYLOAD_BYTES} バイト)"
            ),
            Self::HeaderTooLong => write!(
                formatter,
                "LSP サーバの応答のヘッダが {MAX_HEADER_BYTES} バイトを超えても終わりません"
            ),
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
            | Self::PayloadTooLarge { .. }
            | Self::HeaderTooLong
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
    fn test_payload_of_with_a_header_line_that_never_ends_reports_it() {
        // 改行を送ってこないサーバ。行の長さを縛っていないと、payload の上限に
        // 触れないまま読み続けさせられる
        let framed = "Content-Length: ".to_owned() + &"0".repeat(MAX_HEADER_BYTES);

        let error = read_payload(&framed).expect_err("読めない");

        assert!(matches!(error, FramingError::HeaderTooLong));
    }

    #[test]
    fn test_payload_of_with_headers_that_never_reach_a_blank_line_reports_it() {
        // 1 行ずつは短いが空行に辿り着かないサーバ。行の長さだけを縛っていると素通りする
        let framed = "X: y\r\n".repeat(MAX_HEADER_BYTES);

        let error = read_payload(&framed).expect_err("読めない");

        assert!(matches!(error, FramingError::HeaderTooLong));
    }

    #[test]
    fn test_payload_of_with_a_content_length_over_the_limit_reports_it() {
        // 確保してから気付く実装では、ここで Result ではなくプロセスの死が返る
        let framed = format!("Content-Length: {}\r\n\r\n", MAX_PAYLOAD_BYTES + 1);

        let error = read_payload(&framed).expect_err("読めない");

        assert!(matches!(
            error,
            FramingError::PayloadTooLarge { length } if length == MAX_PAYLOAD_BYTES + 1
        ));
    }

    #[test]
    fn test_payload_of_with_a_content_length_at_the_limit_still_reads() {
        // 上限そのものは受け取る。1 バイト小さく見ていると、正当な応答を弾く
        let payload = "x".repeat(MAX_PAYLOAD_BYTES);
        let framed = format!("Content-Length: {MAX_PAYLOAD_BYTES}\r\n\r\n{payload}");

        assert_eq!(
            read_payload(&framed).expect("読める").len(),
            MAX_PAYLOAD_BYTES
        );
    }

    #[test]
    fn test_payload_of_with_a_non_utf8_body_reports_it() {
        let mut framed = b"Content-Length: 2\r\n\r\n".to_vec();
        framed.extend_from_slice(&[0xff, 0xfe]);

        let error = payload_of(&mut framed.as_slice()).expect_err("読めない");

        assert!(matches!(error, FramingError::NotUtf8));
    }
}
