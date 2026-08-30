//! JSON-RPC のメッセージ。フレームの中身の組み立てと解釈。
//!
//! 区切り（`Content-Length`）は [`super::framing`] の担当で、ここは payload だけを見る。

use std::error::Error;
use std::fmt;

use serde_json::{Value, json};

/// JSON-RPC のバージョン。2.0 以外は送らないし、受け取っても見ない。
const JSON_RPC_VERSION: &str = "2.0";

/// 支えていないメソッドを要求されたときに返すコード（JSON-RPC の Method not found）。
const METHOD_NOT_FOUND: i64 = -32601;

/// こちらが発番する要求の id。
///
/// 応答を要求へ対応付けるためだけに使う。**数で持つのはこちらが数しか発番しないため**で、
/// サーバから届く要求の id（仕様上は文字列もありうる）はここには入らない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestId(u64);

impl RequestId {
    /// 最初の id。
    pub fn first() -> Self {
        Self(1)
    }

    /// 次の id。
    ///
    /// 1 つずつ進める。飽和させるのは、要求を 2^64 回投げる前にプロセスが終わるため。
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    /// id の数そのもの。
    pub fn number(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// サーバが発番した要求の id。
///
/// **こちらの [`RequestId`] と別の型にする。** 仕様上サーバは文字列の id も使えるうえ、
/// この id は突き合わせるためではなく**そのまま返すため**に持つ。
/// 1 つの型にまとめると、対応付けに使えない値が対応付けの側へ流れ込む。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerRequestId {
    /// 数の id。
    Number(i64),
    /// 文字列の id。
    Text(String),
}

impl ServerRequestId {
    /// JSON の id から作る。数でも文字列でもなければ `None`。
    fn from_json(id: &Value) -> Option<Self> {
        if let Some(number) = id.as_i64() {
            return Some(Self::Number(number));
        }

        id.as_str().map(|text| Self::Text(text.to_owned()))
    }

    /// 応答に載せる形に戻す。
    ///
    /// 受け取ったときの種類（数 / 文字列）を保つ。数で来た id を文字列で返すと、
    /// サーバは自分の要求と結び付けられない。
    fn to_json(&self) -> Value {
        match self {
            Self::Number(number) => json!(number),
            Self::Text(text) => json!(text),
        }
    }
}

/// 応答を伴う要求の payload。
///
/// `params` が `None` のときはキーごと省く。`null` を送ると、省略と区別するサーバに
/// 別物として届く。
pub fn request_payload_of(id: RequestId, method: &str, params: Option<Value>) -> String {
    let message = match params {
        Some(params) => json!({
            "jsonrpc": JSON_RPC_VERSION,
            "id": id.number(),
            "method": method,
            "params": params,
        }),
        None => json!({
            "jsonrpc": JSON_RPC_VERSION,
            "id": id.number(),
            "method": method,
        }),
    };

    message.to_string()
}

/// 応答を伴わない通知の payload。
///
/// 要求との違いは id を持たないこと。持たせるとサーバは応答を返そうとする。
pub fn notification_payload_of(method: &str, params: Option<Value>) -> String {
    let message = match params {
        Some(params) => json!({
            "jsonrpc": JSON_RPC_VERSION,
            "method": method,
            "params": params,
        }),
        None => json!({
            "jsonrpc": JSON_RPC_VERSION,
            "method": method,
        }),
    };

    message.to_string()
}

/// サーバからの要求に「受け取った」とだけ返す payload。
///
/// **中身を持たない応答が要る相手がいる。** `window/workDoneProgress/create` は
/// 進捗を作ってよいかを尋ねる要求で、返す値そのものは無い（仕様上 result は null）。
/// [`method_not_found_payload_of`] で断ると、サーバは進捗を送ってこない。
pub fn null_result_payload_of(id: &ServerRequestId) -> String {
    let message = json!({
        "jsonrpc": JSON_RPC_VERSION,
        "id": id.to_json(),
        "result": Value::Null,
    });

    message.to_string()
}

/// サーバからの要求に「そのメソッドは支えていない」と返す payload。
///
/// **支えていないからこそ返す。** 黙って捨てると、応答を待つサーバはそこで止まり、
/// こちらは次のフレームを待つので、双方が待ち合う。
pub fn method_not_found_payload_of(id: &ServerRequestId, method: &str) -> String {
    let message = json!({
        "jsonrpc": JSON_RPC_VERSION,
        "id": id.to_json(),
        "error": {
            "code": METHOD_NOT_FOUND,
            "message": format!("dryguard は {method} を支えていません"),
        },
    });

    message.to_string()
}

/// サーバから届いた 1 通。
///
/// 「id を持つのは応答とサーバからの要求だけ」「method を持つのは要求と通知だけ」を
/// 構造に出す。1 つの型に id と method と result を並べると、どれとも付かない状態が
/// 作れてしまう (rules/coding.md「不正な状態を型で表現できなくする」)。
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    /// こちらの要求への応答。
    Response {
        /// 対応する要求の id。
        id: RequestId,
        /// 成否と、その中身。
        outcome: ResponseOutcome,
    },
    /// サーバからこちらへの要求。
    Request {
        /// 返すときに載せ直す id。
        id: ServerRequestId,
        /// 要求されたメソッド名。
        method: String,
        /// 要求に添えられた値。無ければ `None`。
        params: Option<Value>,
    },
    /// サーバからの通知。
    Notification {
        /// 通知のメソッド名。
        method: String,
        /// 通知に添えられた値。無ければ `None`。
        params: Option<Value>,
    },
}

impl ServerMessage {
    /// 受け取った payload を解釈する。
    ///
    /// # Errors
    ///
    /// JSON として読めないとき、id も method も持たないとき、応答の id が数でないとき、
    /// 要求の id が数でも文字列でもないとき、応答が result も error も持たないとき、
    /// error に code / message が揃っていないとき。
    pub fn from_json(payload: &str) -> Result<Self, MessageError> {
        let message: Value = serde_json::from_str(payload).map_err(MessageError::NotJson)?;
        let method = message.get("method").and_then(Value::as_str);

        match (message.get("id"), method) {
            (Some(id), None) => Ok(Self::Response {
                id: request_id_of(id)?,
                outcome: outcome_of(&message)?,
            }),
            (Some(id), Some(method)) => Ok(Self::Request {
                id: server_request_id_of(id)?,
                method: method.to_owned(),
                params: message.get("params").cloned(),
            }),
            (None, Some(method)) => Ok(Self::Notification {
                method: method.to_owned(),
                params: message.get("params").cloned(),
            }),
            (None, None) => Err(MessageError::Unrecognized),
        }
    }
}

/// 応答が成功だったか、失敗だったか。
///
/// JSON-RPC の応答は result と error のどちらか一方しか持たない。
#[derive(Debug, Clone, PartialEq)]
pub enum ResponseOutcome {
    /// 成功。result の中身。
    Success(Value),
    /// 失敗。
    Failure(ResponseFailure),
}

/// サーバが返した失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseFailure {
    /// JSON-RPC のエラーコード。
    pub code: i64,
    /// サーバが付けた説明。
    pub message: String,
}

impl fmt::Display for ResponseFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} (code {})", self.message, self.code)
    }
}

/// 応答の id を [`RequestId`] にする。
fn request_id_of(id: &Value) -> Result<RequestId, MessageError> {
    id.as_u64()
        .map(RequestId)
        .ok_or_else(|| MessageError::ResponseIdNotANumber { id: id.to_string() })
}

/// サーバからの要求の id を [`ServerRequestId`] にする。
fn server_request_id_of(id: &Value) -> Result<ServerRequestId, MessageError> {
    ServerRequestId::from_json(id)
        .ok_or_else(|| MessageError::RequestIdNotSupported { id: id.to_string() })
}

/// 応答の result / error を [`ResponseOutcome`] にする。
fn outcome_of(message: &Value) -> Result<ResponseOutcome, MessageError> {
    if let Some(failure) = message.get("error") {
        return Ok(ResponseOutcome::Failure(failure_of(failure)?));
    }

    // `"result": null` は shutdown の応答などで正規に来るので、キーの有無で見る。
    let result = message
        .get("result")
        .ok_or(MessageError::ResponseWithoutOutcome)?;

    Ok(ResponseOutcome::Success(result.clone()))
}

/// 応答の error を [`ResponseFailure`] にする。
fn failure_of(failure: &Value) -> Result<ResponseFailure, MessageError> {
    let code = failure.get("code").and_then(Value::as_i64);
    let message = failure.get("message").and_then(Value::as_str);

    match (code, message) {
        (Some(code), Some(message)) => Ok(ResponseFailure {
            code,
            message: message.to_owned(),
        }),
        _ => Err(MessageError::MalformedResponseFailure),
    }
}

/// payload の解釈が失敗した理由。
#[derive(Debug)]
pub enum MessageError {
    /// JSON として読めない。
    NotJson(serde_json::Error),
    /// id も method も無く、応答とも要求とも通知とも読めない。
    Unrecognized,
    /// 応答の id が数ではない。こちらは数しか発番しないので、対応付ける相手がいない。
    ResponseIdNotANumber {
        /// 数として読めなかった id。
        id: String,
    },
    /// サーバからの要求の id が数でも文字列でもない。そのまま返せない。
    RequestIdNotSupported {
        /// 返せなかった id。
        id: String,
    },
    /// 応答が result も error も持たない。
    ResponseWithoutOutcome,
    /// 応答の error に code / message が揃っていない。
    MalformedResponseFailure,
}

impl fmt::Display for MessageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotJson(cause) => {
                write!(formatter, "LSP サーバの応答が JSON ではありません: {cause}")
            }
            Self::Unrecognized => write!(
                formatter,
                "LSP サーバのメッセージに id も method もありません"
            ),
            Self::ResponseIdNotANumber { id } => {
                write!(formatter, "LSP サーバの応答の id が数ではありません: {id}")
            }
            Self::RequestIdNotSupported { id } => write!(
                formatter,
                "LSP サーバの要求の id が数でも文字列でもありません: {id}"
            ),
            Self::ResponseWithoutOutcome => {
                write!(formatter, "LSP サーバの応答に result も error もありません")
            }
            Self::MalformedResponseFailure => write!(
                formatter,
                "LSP サーバの応答の error に code / message がありません"
            ),
        }
    }
}

impl Error for MessageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unrecognized
            | Self::ResponseIdNotANumber { .. }
            | Self::RequestIdNotSupported { .. }
            | Self::ResponseWithoutOutcome
            | Self::MalformedResponseFailure => None,
            Self::NotJson(cause) => Some(cause),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(payload: &str) -> Value {
        serde_json::from_str(payload).expect("組み立てた payload は JSON")
    }

    #[test]
    fn test_request_payload_of_carries_the_id_method_and_params() {
        let payload = request_payload_of(RequestId::first(), "initialize", Some(json!({"a": 1})));

        assert_eq!(
            parsed(&payload),
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"a": 1}})
        );
    }

    #[test]
    fn test_request_payload_of_without_params_omits_the_key() {
        // null を入れると、省略と区別するサーバに別物として届く
        let payload = request_payload_of(RequestId::first(), "shutdown", None);

        assert_eq!(
            parsed(&payload),
            json!({"jsonrpc": "2.0", "id": 1, "method": "shutdown"})
        );
    }

    #[test]
    fn test_notification_payload_of_carries_no_id() {
        // id を持たせるとサーバは応答を返そうとし、こちらは待っていない応答を受け取る
        let payload = notification_payload_of("exit", None);

        assert_eq!(
            parsed(&payload),
            json!({"jsonrpc": "2.0", "method": "exit"})
        );
    }

    #[test]
    fn test_request_id_next_advances_by_one() {
        assert_eq!(RequestId::first().next().number(), 2);
    }

    #[test]
    fn test_server_message_from_a_response_with_a_result_is_a_success() {
        let message = ServerMessage::from_json(r#"{"jsonrpc":"2.0","id":1,"result":{"a":1}}"#)
            .expect("解釈できる");

        assert_eq!(
            message,
            ServerMessage::Response {
                id: RequestId::first(),
                outcome: ResponseOutcome::Success(json!({"a": 1})),
            }
        );
    }

    #[test]
    fn test_server_message_from_a_response_with_a_null_result_is_a_success() {
        // shutdown の応答は result が null。キーの有無ではなく値で見ていると落ちる
        let message = ServerMessage::from_json(r#"{"jsonrpc":"2.0","id":1,"result":null}"#)
            .expect("解釈できる");

        assert_eq!(
            message,
            ServerMessage::Response {
                id: RequestId::first(),
                outcome: ResponseOutcome::Success(Value::Null),
            }
        );
    }

    #[test]
    fn test_server_message_from_a_response_with_an_error_is_a_failure() {
        let payload = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"unknown"}}"#;

        let message = ServerMessage::from_json(payload).expect("解釈できる");

        assert_eq!(
            message,
            ServerMessage::Response {
                id: RequestId::first(),
                outcome: ResponseOutcome::Failure(ResponseFailure {
                    code: -32601,
                    message: "unknown".to_owned(),
                }),
            }
        );
    }

    #[test]
    fn test_server_message_without_an_id_is_a_notification() {
        let payload = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3}}"#;

        let message = ServerMessage::from_json(payload).expect("解釈できる");

        assert_eq!(
            message,
            ServerMessage::Notification {
                method: "window/logMessage".to_owned(),
                params: Some(json!({"type": 3})),
            }
        );
    }

    #[test]
    fn test_server_message_from_a_notification_keeps_the_params_it_carries() {
        // 進捗の終わりは params の中にしか無い。落とすと、どの作業が終わったのかが消える
        let payload = r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"tk","value":{"kind":"end"}}}"#;

        let message = ServerMessage::from_json(payload).expect("解釈できる");

        assert_eq!(
            message,
            ServerMessage::Notification {
                method: "$/progress".to_owned(),
                params: Some(json!({"token": "tk", "value": {"kind": "end"}})),
            }
        );
    }

    #[test]
    fn test_server_message_with_an_id_and_a_method_is_a_request() {
        // サーバからこちらへの要求。応答（id だけ）と取り違えると、待っている応答として拾う
        let payload = r#"{"jsonrpc":"2.0","id":7,"method":"client/registerCapability"}"#;

        let message = ServerMessage::from_json(payload).expect("解釈できる");

        assert_eq!(
            message,
            ServerMessage::Request {
                id: ServerRequestId::Number(7),
                method: "client/registerCapability".to_owned(),
                params: None,
            }
        );
    }

    #[test]
    fn test_server_message_from_a_request_with_a_string_id_keeps_it_as_text() {
        // サーバは文字列の id も使える。数に寄せると、そのまま返せなくなる
        let payload = r#"{"jsonrpc":"2.0","id":"req-1","method":"window/showMessageRequest"}"#;

        let message = ServerMessage::from_json(payload).expect("解釈できる");

        assert_eq!(
            message,
            ServerMessage::Request {
                id: ServerRequestId::Text("req-1".to_owned()),
                method: "window/showMessageRequest".to_owned(),
                params: None,
            }
        );
    }

    #[test]
    fn test_server_message_from_a_request_with_an_id_that_cannot_be_echoed_reports_it() {
        let payload = r#"{"jsonrpc":"2.0","id":[1],"method":"window/showMessageRequest"}"#;

        let error = ServerMessage::from_json(payload).expect_err("解釈できない");

        assert!(matches!(error, MessageError::RequestIdNotSupported { .. }));
    }

    #[test]
    fn test_null_result_payload_of_answers_with_the_id_it_was_given() {
        // 進捗を作ってよいかの要求への返事。断ると（method not found）、
        // サーバは進捗を送ってこない
        let payload = null_result_payload_of(&ServerRequestId::Text("tk-1".to_owned()));

        let message: Value = serde_json::from_str(&payload).expect("JSON として読める");
        assert_eq!(message["id"], json!("tk-1"));
        assert_eq!(message["result"], Value::Null);
    }

    #[test]
    fn test_method_not_found_payload_of_answers_with_the_id_it_was_given() {
        // 数で来た id を文字列で返すと、サーバは自分の要求と結び付けられない
        let payload =
            method_not_found_payload_of(&ServerRequestId::Number(7), "window/showMessageRequest");

        let answer = parsed(&payload);
        assert_eq!(answer["id"], json!(7));
        assert_eq!(answer["error"]["code"], json!(-32601));
    }

    #[test]
    fn test_method_not_found_payload_of_keeps_a_string_id_as_a_string() {
        let payload =
            method_not_found_payload_of(&ServerRequestId::Text("req-1".to_owned()), "any/method");

        assert_eq!(parsed(&payload)["id"], json!("req-1"));
    }

    #[test]
    fn test_server_message_from_a_non_json_payload_reports_it() {
        let error = ServerMessage::from_json("not json").expect_err("解釈できない");

        assert!(matches!(error, MessageError::NotJson(_)));
    }

    #[test]
    fn test_server_message_without_an_id_or_a_method_is_unrecognized() {
        let error = ServerMessage::from_json(r#"{"jsonrpc":"2.0"}"#).expect_err("解釈できない");

        assert!(matches!(error, MessageError::Unrecognized));
    }

    #[test]
    fn test_server_message_from_a_response_without_a_result_or_an_error_reports_it() {
        let error =
            ServerMessage::from_json(r#"{"jsonrpc":"2.0","id":1}"#).expect_err("解釈できない");

        assert!(matches!(error, MessageError::ResponseWithoutOutcome));
    }

    #[test]
    fn test_server_message_from_a_response_with_a_string_id_reports_it() {
        let payload = r#"{"jsonrpc":"2.0","id":"seven","result":null}"#;

        let error = ServerMessage::from_json(payload).expect_err("解釈できない");

        assert!(matches!(
            error,
            MessageError::ResponseIdNotANumber { id } if id == "\"seven\""
        ));
    }

    #[test]
    fn test_server_message_from_a_response_with_an_error_missing_its_code_reports_it() {
        let payload = r#"{"jsonrpc":"2.0","id":1,"error":{"message":"unknown"}}"#;

        let error = ServerMessage::from_json(payload).expect_err("解釈できない");

        assert!(matches!(error, MessageError::MalformedResponseFailure));
    }
}
