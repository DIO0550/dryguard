//! LSP サーバとの往復。要求と応答の対応付けと、ライフサイクル。
//!
//! **どこから読んでどこへ書くかを型引数にしてある。** 子プロセスのパイプでもバイト列でも
//! 同じ経路を通るので、サーバを起動せずに対応付けの振る舞いを確かめられる
//! (rules/tdd.md「`lsp` は『応答を受け取ってから先』を切り出す」)。

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::io::{self, BufRead, Write};

use lsp_types::notification::{
    DidCloseTextDocument, DidOpenTextDocument, Exit, Initialized, Notification as _,
};
use lsp_types::request::{Initialize, Request as _, Shutdown};
use lsp_types::{
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, InitializeResult, ServerCapabilities,
    TextDocumentIdentifier, Uri,
};
use serde_json::{Value, json};

use super::document::SourceDocument;
use super::framing::{self, FramingError};
use super::message::{
    self, MessageError, RequestId, ResponseFailure, ResponseOutcome, ServerMessage,
};
use super::workspace::WorkspaceRoot;

/// `initialize` でサーバに名乗る名前。
const CLIENT_NAME: &str = "dryguard";

/// LSP サーバとの往復。
///
/// 1 本のストリームを要求と応答が行き来するので、id の発番と対応付けをここが持つ。
/// 開いているドキュメントも、**接続が続く限りサーバと共有している状態**なのでここが持つ。
#[derive(Debug)]
pub struct Connection<R: BufRead, W: Write> {
    reader: R,
    writer: W,
    next_id: RequestId,
    open_documents: BTreeSet<Uri>,
}

impl<R: BufRead, W: Write> Connection<R, W> {
    /// 読み口と書き口から往復を組み立てる。
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            next_id: RequestId::first(),
            open_documents: BTreeSet::new(),
        }
    }

    /// サーバと握手し、サーバができることを受け取る。
    ///
    /// `initialize` 要求 → 応答 → `initialized` 通知までで 1 つの握手。**分けて公開しない**のは、
    /// `initialized` を送らないままだとサーバが要求を受け付けず、しかも黙って待つため
    /// (rules/naming.md「`And` を含む名前を作らない」の「2 つをまとめて表す 1 つの概念名」)。
    ///
    /// `root` はサーバに見せるワークスペースの根。開くファイルがどのプロジェクトのものかを
    /// サーバが決める起点になる。
    ///
    /// **Why not（`workspaceFolders` で渡す）**: typescript-language-server は
    /// `rootUri` からしか根を取らない（`workspaceFolders` は根の決定に使っていない）。
    /// 渡すには capabilities での宣言も要るので、宣言しない側の判断とも衝突する。
    ///
    /// # Errors
    ///
    /// 送受信が失敗したとき、応答が `initialize` の結果として読めないとき、
    /// サーバが error を返したとき。
    pub fn handshake(
        &mut self,
        root: &WorkspaceRoot,
    ) -> Result<ServerCapabilities, ConnectionError> {
        // ここだけ [`lsp_types`] の型を通さずに組み立てる。**`rootUri` が非推奨として
        // 印されており**（`workspaceFolders` に置き換わった扱い）、型のまま書くと
        // `-D warnings` に触れる。抑制を足すのは rules/coding.md「禁止事項」で塞いである。
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION") },
            // 何も宣言しない。宣言した機能に応じてサーバはこちらへ要求を投げてくるので、
            // 支えていない機能の要求を呼び込まない。ただし `window/showMessageRequest` の
            // ように宣言に依らず届く要求はあり、それは `response_of` が返す。
            "capabilities": {},
            "rootUri": root.uri().as_str(),
        });

        let result = self.request(Initialize::METHOD, Some(params))?;
        let initialized: InitializeResult =
            serde_json::from_value(result).map_err(|cause| ConnectionError::MalformedResult {
                method: Initialize::METHOD.to_owned(),
                cause,
            })?;

        self.notify(Initialized::METHOD, Some(json!({})))?;

        Ok(initialized.capabilities)
    }

    /// サーバを終わらせる。
    ///
    /// `shutdown` 要求 → 応答 → `exit` 通知までで 1 つの終了手順。`exit` だけを送ると
    /// サーバは終了コードで異常を報告する。
    ///
    /// # Errors
    ///
    /// 送受信が失敗したとき、サーバが error を返したとき。
    pub fn shutdown(&mut self) -> Result<(), ConnectionError> {
        // 応答の result は見ない。仕様上 null で、中身を持たないため。
        //
        // Why not（null であることを検証する）: 非 null を返すサーバを落とすと、
        // `exit` を送る前に抜けてしまう。**終わらせるはずの手順が、終わらせずに帰る。**
        // `handshake` が result を読むのは、そちらの中身が要るからで、ここには無い。
        self.request(Shutdown::METHOD, None)?;
        self.notify(Exit::METHOD, None)
    }

    /// 候補ペアのファイルをサーバに開かせる。
    ///
    /// 既に開いているドキュメントには送らない。**候補ペアは同じファイルを何度も指す**
    /// （1 つのファイルに複数のチャンクがある）ので、素直に送ると同じ URI への `didOpen` が
    /// 重なる。LSP は既に開いているドキュメントへの `didOpen` の扱いを定めていない。
    ///
    /// # Errors
    ///
    /// パラメータを JSON にできないとき、送信が失敗したとき。
    pub fn open_document(&mut self, document: &SourceDocument) -> Result<(), ConnectionError> {
        if self.open_documents.contains(document.uri()) {
            return Ok(());
        }

        let params = DidOpenTextDocumentParams {
            text_document: document.to_text_document_item(),
        };
        let params = serde_json::to_value(params)
            .map_err(not_serializable_error_of(DidOpenTextDocument::METHOD))?;
        self.notify(DidOpenTextDocument::METHOD, Some(params))?;

        // 送れてから覚える。送れていないものを開いていることにすると、
        // 開いていないドキュメントへ `didClose` を送る。
        self.open_documents.insert(document.uri().clone());

        Ok(())
    }

    /// 開かせたファイルを閉じさせる。開いていなければ何もしない。
    ///
    /// # Errors
    ///
    /// パラメータを JSON にできないとき、送信が失敗したとき。
    pub fn close_document(&mut self, document: &SourceDocument) -> Result<(), ConnectionError> {
        if !self.open_documents.contains(document.uri()) {
            return Ok(());
        }

        let params = DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier {
                uri: document.uri().clone(),
            },
        };
        let params = serde_json::to_value(params)
            .map_err(not_serializable_error_of(DidCloseTextDocument::METHOD))?;
        self.notify(DidCloseTextDocument::METHOD, Some(params))?;

        self.open_documents.remove(document.uri());

        Ok(())
    }

    /// 要求を送り、その応答の result を待つ。
    ///
    /// # Errors
    ///
    /// 送受信が失敗したとき、待っている id と違う応答が届いたとき、
    /// サーバが error を返したとき。
    fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, ConnectionError> {
        let id = self.next_id;
        self.next_id = id.next();

        self.send(&message::request_payload_of(id, method, params))?;
        self.response_of(id, method)
    }

    /// 通知を送る。応答は待たない。
    ///
    /// # Errors
    ///
    /// 送信が失敗したとき。
    fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), ConnectionError> {
        self.send(&message::notification_payload_of(method, params))
    }

    /// `id` の応答が届くまで読み、その result を返す。
    ///
    /// `method` は失敗を報告するときに、どの要求が落ちたのかを出すために持つ。
    ///
    /// # Errors
    ///
    /// 読み取りが失敗したとき、待っている id と違う応答が届いたとき、
    /// サーバが error を返したとき。
    fn response_of(&mut self, id: RequestId, method: &str) -> Result<Value, ConnectionError> {
        loop {
            let payload =
                framing::payload_of(&mut self.reader).map_err(ConnectionError::Framing)?;
            let message = ServerMessage::from_json(&payload).map_err(ConnectionError::Message)?;

            let (responded, outcome) = match message {
                // 応答の前後にサーバの通知（window/logMessage・$/progress）が挟まる。
                // 通知に応答は要らないので、次のフレームへ進むだけでよい。
                ServerMessage::Notification { .. } => continue,
                // 要求には返す。黙って捨てると、応答を待つサーバはそこで止まり、
                // こちらは次のフレームを待つので、双方が待ち合う。
                ServerMessage::Request { id, method } => {
                    self.send(&message::method_not_found_payload_of(&id, &method))?;
                    continue;
                }
                ServerMessage::Response { id, outcome } => (id, outcome),
            };

            if responded != id {
                return Err(ConnectionError::UnexpectedResponse {
                    expected: id,
                    received: responded,
                });
            }

            return match outcome {
                ResponseOutcome::Success(result) => Ok(result),
                ResponseOutcome::Failure(failure) => Err(ConnectionError::ServerFailure {
                    method: method.to_owned(),
                    failure,
                }),
            };
        }
    }

    /// payload を 1 フレームとして書き出す。
    ///
    /// # Errors
    ///
    /// 書き出しが失敗したとき。
    fn send(&mut self, payload: &str) -> Result<(), ConnectionError> {
        self.writer
            .write_all(&framing::framed_bytes_of(payload))
            .map_err(ConnectionError::Send)?;

        // 都度流す。パイプの向こうは応答を返すまでこちらの続きを読まないので、
        // 溜めたままだと双方が待ち合う。
        self.writer.flush().map_err(ConnectionError::Send)
    }
}

/// パラメータを JSON にできなかったときの理由を、そのメソッド名で組み立てる。
fn not_serializable_error_of(
    method: &'static str,
) -> impl Fn(serde_json::Error) -> ConnectionError {
    move |cause| ConnectionError::ParamsNotSerializable {
        method: method.to_owned(),
        cause,
    }
}

/// 往復が失敗した理由。
#[derive(Debug)]
pub enum ConnectionError {
    /// フレームを読めなかった。
    Framing(FramingError),
    /// フレームの中身を解釈できなかった。
    Message(MessageError),
    /// 書き出せなかった。
    Send(io::Error),
    /// 待っている id と違う応答が届いた。
    UnexpectedResponse {
        /// 待っていた id。
        expected: RequestId,
        /// 届いた応答の id。
        received: RequestId,
    },
    /// サーバが error を返した。
    ServerFailure {
        /// 失敗した要求のメソッド名。
        method: String,
        /// サーバが返した内容。
        failure: ResponseFailure,
    },
    /// 応答の result を、そのメソッドの結果として読めない。
    MalformedResult {
        /// 読めなかった応答のメソッド名。
        method: String,
        /// 読めなかった理由。
        cause: serde_json::Error,
    },
    /// 送るパラメータを JSON にできない。
    ParamsNotSerializable {
        /// 送ろうとした要求のメソッド名。
        method: String,
        /// できなかった理由。
        cause: serde_json::Error,
    },
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Framing(cause) => write!(formatter, "{cause}"),
            Self::Message(cause) => write!(formatter, "{cause}"),
            Self::Send(cause) => write!(formatter, "LSP サーバへ送れません: {cause}"),
            Self::UnexpectedResponse { expected, received } => write!(
                formatter,
                "LSP サーバの応答の id が違います (待っていた id: {expected} / 届いた id: {received})"
            ),
            Self::ServerFailure { method, failure } => {
                write!(formatter, "LSP サーバが {method} を拒みました: {failure}")
            }
            Self::MalformedResult { method, cause } => write!(
                formatter,
                "LSP サーバの {method} の応答を読めません: {cause}"
            ),
            Self::ParamsNotSerializable { method, cause } => write!(
                formatter,
                "{method} のパラメータを JSON にできません: {cause}"
            ),
        }
    }
}

impl Error for ConnectionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnexpectedResponse { .. } | Self::ServerFailure { .. } => None,
            Self::Framing(cause) => Some(cause),
            Self::Message(cause) => Some(cause),
            Self::Send(cause) => Some(cause),
            Self::MalformedResult { cause, .. } | Self::ParamsNotSerializable { cause, .. } => {
                Some(cause)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::repository_path;
    use lsp_types::HoverProviderCapability;

    /// テストが渡すワークスペースの根。実在するディレクトリからしか作れない。
    fn workspace_root() -> WorkspaceRoot {
        WorkspaceRoot::enclosing(&[repository_path("src/lib.rs")]).expect("根を決められる")
    }

    /// テストが開かせるドキュメント。実在するファイルからしか作れない。
    fn document_of(relative: &str, text: &str) -> SourceDocument {
        SourceDocument::new(&repository_path(relative), text.to_owned())
            .expect("ドキュメントにできる")
    }

    /// payload をフレームに並べたバイト列。サーバが吐く側として使う。
    fn frames_of(payloads: &[&str]) -> Vec<u8> {
        payloads
            .iter()
            .flat_map(|payload| framing::framed_bytes_of(payload))
            .collect()
    }

    /// 書き出されたフレームを順に解いたもの。
    fn sent_payloads(written: &[u8]) -> Vec<Value> {
        let mut reader = written;
        let mut payloads = Vec::new();

        while let Ok(payload) = framing::payload_of(&mut reader) {
            payloads.push(serde_json::from_str(&payload).expect("送った payload は JSON"));
        }

        payloads
    }

    /// 書き出されたフレームから method 名を順に取り出す。応答は method を持たないので飛ばす。
    fn sent_methods(written: &[u8]) -> Vec<String> {
        sent_payloads(written)
            .iter()
            .filter_map(|payload| payload["method"].as_str().map(str::to_owned))
            .collect()
    }

    /// 書き出されたフレームから id を順に取り出す。通知は id を持たないので飛ばす。
    fn sent_ids(written: &[u8]) -> Vec<u64> {
        sent_payloads(written)
            .iter()
            .filter_map(|payload| payload["id"].as_u64())
            .collect()
    }

    fn connection_over(server_output: &[u8]) -> Connection<&[u8], Vec<u8>> {
        Connection::new(server_output, Vec::new())
    }

    #[test]
    fn test_request_returns_the_result_of_the_matching_response() {
        let server_output = frames_of(&[r#"{"jsonrpc":"2.0","id":1,"result":{"a":1}}"#]);
        let mut connection = connection_over(&server_output);

        let result = connection
            .request("initialize", None)
            .expect("応答を受け取れる");

        assert_eq!(result, json!({"a": 1}));
    }

    #[test]
    fn test_request_skips_notifications_arriving_before_the_response() {
        // 通知を応答と取り違えていると、ここで id の突き合わせに失敗する
        let server_output = frames_of(&[
            r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3}}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":{"a":1}}"#,
        ]);
        let mut connection = connection_over(&server_output);

        let result = connection
            .request("initialize", None)
            .expect("応答を受け取れる");

        assert_eq!(result, json!({"a": 1}));
    }

    #[test]
    fn test_request_skips_a_server_request_arriving_before_the_response() {
        // サーバからの要求は id を持つ。応答と取り違えると、待っている結果として拾う
        let server_output = frames_of(&[
            r#"{"jsonrpc":"2.0","id":9,"method":"client/registerCapability","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":{"a":1}}"#,
        ]);
        let mut connection = connection_over(&server_output);

        let result = connection
            .request("initialize", None)
            .expect("応答を受け取れる");

        assert_eq!(result, json!({"a": 1}));
    }

    #[test]
    fn test_request_answers_a_server_request_before_taking_its_own_response() {
        // 返さないと、応答を待つサーバはそこで止まり、こちらは次のフレームを待つ
        let server_output = frames_of(&[
            r#"{"jsonrpc":"2.0","id":9,"method":"window/showMessageRequest","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":{"a":1}}"#,
        ]);
        let mut connection = connection_over(&server_output);

        connection
            .request("initialize", None)
            .expect("応答を受け取れる");

        let sent = sent_payloads(&connection.writer);
        assert_eq!(sent.len(), 2, "要求 1 通と、サーバの要求への応答 1 通");
        assert_eq!(sent[1]["id"], json!(9), "サーバの id をそのまま返す");
        assert_eq!(sent[1]["error"]["code"], json!(-32601));
    }

    #[test]
    fn test_request_with_a_response_for_another_id_reports_it() {
        let server_output = frames_of(&[r#"{"jsonrpc":"2.0","id":9,"result":{"a":1}}"#]);
        let mut connection = connection_over(&server_output);

        let error = connection
            .request("initialize", None)
            .expect_err("対応付けられない");

        assert!(matches!(
            error,
            ConnectionError::UnexpectedResponse { expected, received }
                if expected == RequestId::first() && received.number() == 9
        ));
    }

    #[test]
    fn test_request_with_an_error_response_reports_the_server_failure() {
        let server_output = frames_of(&[
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"unknown method"}}"#,
        ]);
        let mut connection = connection_over(&server_output);

        let error = connection.request("hover", None).expect_err("失敗が返る");

        assert!(matches!(
            error,
            ConnectionError::ServerFailure { method, failure }
                if method == "hover" && failure.code == -32601
        ));
    }

    #[test]
    fn test_request_advances_the_id_for_the_next_request() {
        let server_output = frames_of(&[
            r#"{"jsonrpc":"2.0","id":1,"result":null}"#,
            r#"{"jsonrpc":"2.0","id":2,"result":null}"#,
        ]);
        let mut connection = connection_over(&server_output);

        connection.request("shutdown", None).expect("1 回目の応答");
        connection.request("shutdown", None).expect("2 回目の応答");

        assert_eq!(sent_ids(&connection.writer), vec![1, 2]);
    }

    #[test]
    fn test_handshake_returns_the_server_capabilities() {
        let server_output = frames_of(&[
            r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"hoverProvider":true}}}"#,
        ]);
        let mut connection = connection_over(&server_output);

        let capabilities = connection.handshake(&workspace_root()).expect("握手できる");

        assert_eq!(
            capabilities.hover_provider,
            Some(HoverProviderCapability::Simple(true))
        );
    }

    #[test]
    fn test_handshake_sends_initialized_after_the_initialize_response() {
        // 先に initialized を送っていると、ここで順番が入れ替わる
        let server_output =
            frames_of(&[r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#]);
        let mut connection = connection_over(&server_output);

        connection.handshake(&workspace_root()).expect("握手できる");

        assert_eq!(
            sent_methods(&connection.writer),
            vec!["initialize", "initialized"]
        );
    }

    #[test]
    fn test_handshake_with_a_result_that_is_not_an_initialize_result_reports_it() {
        let server_output = frames_of(&[r#"{"jsonrpc":"2.0","id":1,"result":3}"#]);
        let mut connection = connection_over(&server_output);

        let error = connection
            .handshake(&workspace_root())
            .expect_err("読めない");

        assert!(matches!(
            error,
            ConnectionError::MalformedResult { method, .. } if method == "initialize"
        ));
    }

    #[test]
    fn test_handshake_names_the_workspace_root_as_the_root_uri() {
        // 根を渡していないと、サーバは開いたファイルがどのプロジェクトのものか決められない
        let server_output =
            frames_of(&[r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#]);
        let mut connection = connection_over(&server_output);
        let root = workspace_root();

        connection.handshake(&root).expect("握手できる");

        let sent = sent_payloads(&connection.writer);
        assert_eq!(sent[0]["params"]["rootUri"], json!(root.uri().as_str()));
    }

    #[test]
    fn test_open_document_sends_what_the_server_needs_to_read_it() {
        let mut connection = connection_over(&[]);
        let document = document_of("tests/fixtures/billing/discount.ts", "export const a = 1;");

        connection.open_document(&document).expect("送れる");

        let sent = sent_payloads(&connection.writer);
        assert_eq!(sent.len(), 1, "didOpen 1 通");
        assert_eq!(sent[0]["method"], json!("textDocument/didOpen"));
        let opened = &sent[0]["params"]["textDocument"];
        assert_eq!(opened["uri"], json!(document.uri().as_str()));
        assert_eq!(opened["languageId"], json!("typescript"));
        assert_eq!(opened["version"], json!(1));
        assert_eq!(opened["text"], json!("export const a = 1;"));
    }

    #[test]
    fn test_open_document_that_is_already_open_sends_nothing() {
        // 候補ペアは同じファイルを何度も指す。素直に送ると didOpen が重なる
        let mut connection = connection_over(&[]);
        let opened_twice = document_of("tests/fixtures/billing/discount.ts", "export const a = 1;");
        let another = document_of("tests/fixtures/inventory/reorder.ts", "export const b = 2;");

        connection.open_document(&opened_twice).expect("送れる");
        connection.open_document(&opened_twice).expect("送れる");
        connection.open_document(&another).expect("送れる");

        let sent = sent_payloads(&connection.writer);
        let opened_uris: Vec<&Value> = sent
            .iter()
            .map(|payload| &payload["params"]["textDocument"]["uri"])
            .collect();
        assert_eq!(
            opened_uris,
            vec![
                &json!(opened_twice.uri().as_str()),
                &json!(another.uri().as_str())
            ]
        );
    }

    #[test]
    fn test_close_document_names_the_document_it_closes() {
        let mut connection = connection_over(&[]);
        let document = document_of("tests/fixtures/billing/discount.ts", "export const a = 1;");
        connection.open_document(&document).expect("送れる");

        connection.close_document(&document).expect("送れる");

        let sent = sent_payloads(&connection.writer);
        assert_eq!(sent.len(), 2, "didOpen と didClose の 2 通");
        assert_eq!(sent[1]["method"], json!("textDocument/didClose"));
        assert_eq!(
            sent[1]["params"]["textDocument"]["uri"],
            json!(document.uri().as_str())
        );
    }

    #[test]
    fn test_close_document_that_is_not_open_sends_nothing() {
        // 開いていないドキュメントを閉じさせると、サーバは知らない URI を受け取る
        let mut connection = connection_over(&[]);
        let opened = document_of("tests/fixtures/billing/discount.ts", "export const a = 1;");
        let never_opened =
            document_of("tests/fixtures/inventory/reorder.ts", "export const b = 2;");
        connection.open_document(&opened).expect("送れる");

        connection.close_document(&never_opened).expect("送れる");
        connection.close_document(&opened).expect("送れる");

        let closed_uris: Vec<Value> = sent_payloads(&connection.writer)
            .iter()
            .filter(|payload| payload["method"] == json!("textDocument/didClose"))
            .map(|payload| payload["params"]["textDocument"]["uri"].clone())
            .collect();
        assert_eq!(closed_uris, vec![json!(opened.uri().as_str())]);
    }

    #[test]
    fn test_close_document_after_it_was_closed_sends_nothing() {
        // 閉じた記録が残っていると、2 回目の didClose がそのまま出ていく
        let mut connection = connection_over(&[]);
        let document = document_of("tests/fixtures/billing/discount.ts", "export const a = 1;");
        connection.open_document(&document).expect("送れる");
        connection.close_document(&document).expect("送れる");

        connection.close_document(&document).expect("送れる");

        assert_eq!(
            sent_methods(&connection.writer),
            vec!["textDocument/didOpen", "textDocument/didClose"]
        );
    }

    #[test]
    fn test_open_document_after_it_was_closed_sends_it_again() {
        // 開いた記録が残っていると、閉じた後に開き直せない
        let mut connection = connection_over(&[]);
        let document = document_of("tests/fixtures/billing/discount.ts", "export const a = 1;");
        connection.open_document(&document).expect("送れる");
        connection.close_document(&document).expect("送れる");

        connection.open_document(&document).expect("送れる");

        assert_eq!(
            sent_methods(&connection.writer),
            vec![
                "textDocument/didOpen",
                "textDocument/didClose",
                "textDocument/didOpen"
            ]
        );
    }

    #[test]
    fn test_shutdown_sends_exit_after_the_shutdown_response() {
        // exit だけを先に送るとサーバは異常終了として扱う
        let server_output = frames_of(&[r#"{"jsonrpc":"2.0","id":1,"result":null}"#]);
        let mut connection = connection_over(&server_output);

        connection.shutdown().expect("終了できる");

        assert_eq!(sent_methods(&connection.writer), vec!["shutdown", "exit"]);
    }

    #[test]
    fn test_request_after_the_server_closed_reports_it() {
        let mut connection = connection_over(&[]);

        let error = connection
            .request("shutdown", None)
            .expect_err("応答が無い");

        assert!(matches!(
            error,
            ConnectionError::Framing(FramingError::ServerClosed)
        ));
    }
}
