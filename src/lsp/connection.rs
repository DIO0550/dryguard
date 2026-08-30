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
    DidCloseTextDocument, DidOpenTextDocument, Exit, Initialized, Notification as _, Progress,
};
use lsp_types::request::{
    HoverRequest, Initialize, References, Request as _, Shutdown, WorkDoneProgressCreate,
};
use lsp_types::{
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, Hover, HoverParams, InitializeResult,
    Location, PartialResultParams, ProgressParams, ProgressParamsValue, ProgressToken,
    ReferenceContext, ReferenceParams, ServerCapabilities, TextDocumentIdentifier,
    TextDocumentPositionParams, Uri, WorkDoneProgress, WorkDoneProgressCreateParams,
    WorkDoneProgressParams,
};
use serde_json::{Value, json};

use super::document::SourceDocument;
use super::framing::{self, FramingError};
use super::hover::{self, HoverOutcome};
use super::message::{
    self, MessageError, RequestId, ResponseFailure, ResponseOutcome, ServerMessage, ServerRequestId,
};
use super::references::{self, ReferencesOutcome};
use super::workspace::WorkspaceRoot;
use crate::source_position::SourcePosition;

/// `initialize` でサーバに名乗る名前。
const CLIENT_NAME: &str = "dryguard";

/// references を尋ねる回数の上限。
///
/// 1 回目は読み込みの途中に当たりうる。2 回目はその読み込みが終わってからになる。
/// **3 回目でも作業に触れているなら、尋ねるたびに始まっている**ので、待っても
/// 答えは落ち着かない（`ReferencesOutcome::ServerStillWorking`）。
const REFERENCES_ATTEMPTS: usize = 3;

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
    /// 始まりの通知が届き、終わりの通知がまだ来ていない作業。
    running_progress: Vec<ProgressToken>,
    /// サーバの作業について知らされた回数（token を用意する要求と、始まりの通知）。
    ///
    /// **減らさない。** 始まって終わった作業は [`Self::running_progress`] から消えるので、
    /// 尋ねている間に何か起きたかは数でしか追えない。
    noticed_progress_count: usize,
}

impl<R: BufRead, W: Write> Connection<R, W> {
    /// 読み口と書き口から往復を組み立てる。
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            next_id: RequestId::first(),
            open_documents: BTreeSet::new(),
            running_progress: Vec::new(),
            noticed_progress_count: 0,
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
            // 宣言するのは進捗を受け取ることだけ。宣言した機能に応じてサーバはこちらへ
            // 要求を投げてくるので、支えていない機能の要求を呼び込まない。ただし
            // `window/showMessageRequest` のように宣言に依らず届く要求はあり、
            // それは `answer` が断る。
            //
            // Why（進捗だけは宣言する）: **サーバは読み込みの途中でも要求に答える。**
            // typescript-language-server はプロジェクトを読み終える前の
            // `textDocument/references` に空の答えを返すので、宣言しないと
            // 「呼び出し元が無い」と「まだ読んでいない」を区別できない。
            "capabilities": { "window": { "workDoneProgress": true } },
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

    /// 開かせたファイルの、指定位置にある名前の型の綴りを尋ねる。
    ///
    /// `position` は `Chunk::name_position` が指す識別子の位置。答えが無かったのか
    /// 読めなかったのかは [`HoverOutcome`] が分けて持つ。
    ///
    /// # Errors
    ///
    /// そのドキュメントを開かせていないとき、パラメータを JSON にできないとき、
    /// 送受信が失敗したとき、応答を hover の結果として読めないとき。
    pub fn hover(
        &mut self,
        document: &SourceDocument,
        position: SourcePosition,
    ) -> Result<HoverOutcome, ConnectionError> {
        // 開かせていないドキュメントへ送ると、サーバは中身を知らないまま null を返す。
        // 「その位置に型が無い」と「開かせ忘れ」が同じ答えになるので、送る前に断る。
        if !self.open_documents.contains(document.uri()) {
            return Err(ConnectionError::DocumentNotOpen {
                uri: document.uri().clone(),
            });
        }

        let params = HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: document.uri().clone(),
                },
                position: position.to_lsp_position(),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let params = serde_json::to_value(params)
            .map_err(not_serializable_error_of(HoverRequest::METHOD))?;

        let result = self.request(HoverRequest::METHOD, Some(params))?;
        // 答えが無いときサーバは null を返す。`Option` で受けて、応答が読めなかった
        // 場合と分ける。
        let answered: Option<Hover> =
            serde_json::from_value(result).map_err(|cause| ConnectionError::MalformedResult {
                method: HoverRequest::METHOD.to_owned(),
                cause,
            })?;

        Ok(match answered.as_ref() {
            Some(hover) => hover::outcome_of(hover),
            None => HoverOutcome::NoAnswer,
        })
    }

    /// 開かせたファイルの、指定位置にある名前を参照しているところを尋ねる。
    ///
    /// `position` は `Chunk::name_position` が指す識別子の位置。返るのは参照元の
    /// ファイルで、読めなかったのか 1 件も無かったのかは [`ReferencesOutcome`] が分けて持つ。
    ///
    /// **宣言そのものは数えない**（`include_declaration` を立てない）。数えると、
    /// 呼び出し元が 1 件も無いチャンクでも自分の宣言だけが返り、**自分のドメインに
    /// 呼ばれている**ように読める。
    ///
    /// 答えを採るのは**サーバの作業が動いていないとき**だけ。動いている間の答えは
    /// まだ見ていないファイルの分が抜けており、それを最終的なシグナルとして扱うと、
    /// 呼び出し元の分布が実際より狭く出る。
    ///
    /// # Errors
    ///
    /// そのドキュメントを開かせていないとき、パラメータを JSON にできないとき、
    /// 送受信が失敗したとき、応答を references の結果として読めないとき。
    pub fn references(
        &mut self,
        document: &SourceDocument,
        position: SourcePosition,
    ) -> Result<ReferencesOutcome, ConnectionError> {
        // 開かせていないドキュメントへ送ると、サーバは中身を知らないまま空を返す。
        // 「参照元が無い」と「開かせ忘れ」が同じ答えになるので、送る前に断る。
        if !self.open_documents.contains(document.uri()) {
            return Err(ConnectionError::DocumentNotOpen {
                uri: document.uri().clone(),
            });
        }

        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: document.uri().clone(),
                },
                position: position.to_lsp_position(),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext {
                include_declaration: false,
            },
        };
        let params =
            serde_json::to_value(params).map_err(not_serializable_error_of(References::METHOD))?;

        // 読み込み中に返ってきた答えは、**サーバがまだ見ていないファイルの分が抜けている**
        // （呼び出し元は呼び出し先を import する側なので、開かせたファイルからは辿れない）。
        // 作業が終わってから尋ね直し、**作業に触れていない答えだけを採る**。
        let mut attempts_left = REFERENCES_ATTEMPTS;

        while attempts_left > 0 {
            // **尋ねる前に、動いている作業を待ち切る。** 動いている最中に送ると、
            // その作業が終わってから応答が届いたときに、こちらからは何も起きなかったように
            // 見える（前の問い合わせで覚えた作業なので、数も増えない）。
            self.wait_for_running_progress()?;

            let noticed_before = self.noticed_progress_count;
            let answered = self.ask_references_once(params.clone())?;

            // 「今動いているか」だけでは足りない。**尋ねている間に始まって終わった作業**が
            // あると、読み込み前に計算された答えを受け取りながら、手元では何も動いて
            // いないように見える（実測: 進捗の終わりが応答より先に届き、答えは 0 件）。
            let noticed_while_answering = self.noticed_progress_count != noticed_before;
            let untouched_by_work = !noticed_while_answering && self.running_progress.is_empty();
            if untouched_by_work {
                return Ok(answered);
            }

            attempts_left -= 1;
        }

        Ok(ReferencesOutcome::ServerStillWorking)
    }

    /// references を 1 往復だけ尋ねる。
    ///
    /// # Errors
    ///
    /// 送受信が失敗したとき、応答を references の結果として読めないとき。
    fn ask_references_once(&mut self, params: Value) -> Result<ReferencesOutcome, ConnectionError> {
        let result = self.request(References::METHOD, Some(params))?;
        // 参照元が無いときサーバは null を返す。`Option` で受けて、応答が読めなかった
        // 場合と分ける。
        let answered: Option<Vec<Location>> =
            serde_json::from_value(result).map_err(|cause| ConnectionError::MalformedResult {
                method: References::METHOD.to_owned(),
                cause,
            })?;

        Ok(match answered.as_deref() {
            Some(locations) => references::outcome_of(locations),
            None => ReferencesOutcome::NoAnswer,
        })
    }

    /// サーバが始めた作業がすべて終わるまで、届くメッセージを読み進める。
    ///
    /// 待つのは**始まったと分かっている作業だけ**。始まっていないものを待つと、
    /// 何も送ってこないサーバの前で止まる（待ちに期限を設ける話は Issue #108）。
    ///
    /// # Errors
    ///
    /// 読み取りが失敗したとき、要求していない応答が届いたとき。
    fn wait_for_running_progress(&mut self) -> Result<(), ConnectionError> {
        while !self.running_progress.is_empty() {
            if let Some(responded) = self.handled_message()? {
                return Err(ConnectionError::UnrequestedResponse {
                    received: responded.0,
                });
            }
        }

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
            let Some((responded, outcome)) = self.handled_message()? else {
                continue;
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

    /// 次の 1 通を読み、応答でなければその場で捌く。
    ///
    /// 応答だったときだけ、その id と中身を返す。
    ///
    /// # Errors
    ///
    /// 読み取りが失敗したとき、中身を解釈できないとき、要求への返信を書き出せないとき。
    fn handled_message(&mut self) -> Result<Option<(RequestId, ResponseOutcome)>, ConnectionError> {
        let payload = framing::payload_of(&mut self.reader).map_err(ConnectionError::Framing)?;
        let message = ServerMessage::from_json(&payload).map_err(ConnectionError::Message)?;

        match message {
            // 応答の前後にサーバの通知（window/logMessage・$/progress）が挟まる。
            // 通知に応答は要らないが、進捗の終わりだけは覚える。
            ServerMessage::Notification { method, params } => {
                self.note_progress(&method, params.as_ref())?;
                Ok(None)
            }
            // 要求には返す。黙って捨てると、応答を待つサーバはそこで止まり、
            // こちらは次のフレームを待つので、双方が待ち合う。
            ServerMessage::Request { id, method, params } => {
                self.answer(&id, &method, params.as_ref())?;
                Ok(None)
            }
            ServerMessage::Response { id, outcome } => Ok(Some((id, outcome))),
        }
    }

    /// サーバからの要求に返す。
    ///
    /// 進捗を作ってよいかの要求だけは受ける（受けないと進捗が届かない）。
    /// **それ以外は支えていないと返す。**
    ///
    /// 受けた時点では**待つ対象にしない**。この要求は token を用意するだけで、
    /// 作業が始まったことを意味しない。始まらないまま捨てられた token を待つと、
    /// 終わりの来ない作業を待ち続ける。始まりを覚えるのは [`Self::note_progress`]。
    ///
    /// # Errors
    ///
    /// 進捗の要求から token を読めないとき、書き出しが失敗したとき。
    fn answer(
        &mut self,
        id: &ServerRequestId,
        method: &str,
        params: Option<&Value>,
    ) -> Result<(), ConnectionError> {
        if method != WorkDoneProgressCreate::METHOD {
            return self.send(&message::method_not_found_payload_of(id, method));
        }

        // token を覚えるのは始まりの通知のほうなので、ここでは読めることだけを確かめる。
        // 読めない要求を黙って受けると、後から届く進捗と突き合わせられない。
        let _created: WorkDoneProgressCreateParams = serde_json::from_value(params_or_null(params))
            .map_err(malformed_params_error_of(WorkDoneProgressCreate::METHOD))?;

        self.send(&message::null_result_payload_of(id))?;

        // 数はここでも進める。**作業が起きうると分かるのはこの時点**で、尋ねている間に
        // ここを通ったかどうかが、答えを信用してよいかの判断材料になる
        // （`running_progress` は始まって終わると空に戻り、痕跡が残らない）。
        self.noticed_progress_count = self.noticed_progress_count.saturating_add(1);

        Ok(())
    }

    /// 動いている作業を、始まりと終わりの通知で覚える。進捗でない通知は見ない。
    ///
    /// **待つ対象にするのは始まりの通知から。** token を作ってよいかの要求
    /// （[`Self::answer`]）は用意するだけで、そのまま始まらないこともある。
    ///
    /// # Errors
    ///
    /// 進捗の通知から token と中身を読めないとき。
    fn note_progress(
        &mut self,
        method: &str,
        params: Option<&Value>,
    ) -> Result<(), ConnectionError> {
        if method != Progress::METHOD {
            return Ok(());
        }

        let progress: ProgressParams = serde_json::from_value(params_or_null(params))
            .map_err(malformed_params_error_of(Progress::METHOD))?;
        let ProgressParamsValue::WorkDone(work) = progress.value;

        match work {
            // **始まりでも数を進める。** token を用意するのが前の問い合わせの最中で、
            // 始まりと終わりだけが次の問い合わせの最中に届く並びがある。用意した時点しか
            // 数えないと、その並びで「何も起きなかった」ように見える。
            WorkDoneProgress::Begin(_) => {
                self.noticed_progress_count = self.noticed_progress_count.saturating_add(1);
                self.running_progress.push(progress.token);
            }
            WorkDoneProgress::End(_) => self
                .running_progress
                .retain(|token| *token != progress.token),
            // 途中経過は動いていることを言い直しているだけで、待つ相手は変わらない。
            WorkDoneProgress::Report(_) => {}
        }

        Ok(())
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

/// サーバが添えた値。付いていなければ `null`。
///
/// **付いていないことを別扱いにしない。** 読む相手（進捗）はどれも値を要求するので、
/// 無いのと読めないのは同じ「読めません」に落ちる。
fn params_or_null(params: Option<&Value>) -> Value {
    params.cloned().unwrap_or(Value::Null)
}

/// サーバが添えた値を読めなかったときの理由を、そのメソッド名で組み立てる。
fn malformed_params_error_of(
    method: &'static str,
) -> impl Fn(serde_json::Error) -> ConnectionError {
    move |cause| ConnectionError::MalformedParams {
        method: method.to_owned(),
        cause,
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
    /// 要求していない応答が届いた。
    ///
    /// 進捗の終わりを待っている間はこちらの要求が 1 つも出ていないので、
    /// 応答が来ること自体が食い違いになる。
    UnrequestedResponse {
        /// 届いた応答の id。
        received: RequestId,
    },
    /// サーバが添えた値を、そのメソッドのパラメータとして読めない。
    MalformedParams {
        /// 読めなかった要求・通知のメソッド名。
        method: String,
        /// 読めなかった理由。
        cause: serde_json::Error,
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
    /// 開かせていないドキュメントの位置を問い合わせようとした。
    ///
    /// サーバは知らない URI に対して null を返すので、送ってしまうと
    /// 「その位置に型が無い」と区別が付かない。
    DocumentNotOpen {
        /// 開かせていなかったドキュメントの URI。
        uri: Uri,
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
            Self::UnrequestedResponse { received } => {
                write!(formatter, "要求していない応答が届きました (id {received})")
            }
            Self::MalformedParams { method, cause } => {
                write!(formatter, "{method} に添えられた値を読めません: {cause}")
            }
            Self::MalformedResult { method, cause } => write!(
                formatter,
                "LSP サーバの {method} の応答を読めません: {cause}"
            ),
            Self::ParamsNotSerializable { method, cause } => write!(
                formatter,
                "{method} のパラメータを JSON にできません: {cause}"
            ),
            Self::DocumentNotOpen { uri } => write!(
                formatter,
                "LSP サーバに開かせていないドキュメントです: {}",
                uri.as_str()
            ),
        }
    }
}

impl Error for ConnectionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnexpectedResponse { .. }
            | Self::UnrequestedResponse { .. }
            | Self::ServerFailure { .. }
            | Self::DocumentNotOpen { .. } => None,
            Self::Framing(cause) => Some(cause),
            Self::Message(cause) => Some(cause),
            Self::Send(cause) => Some(cause),
            Self::MalformedResult { cause, .. }
            | Self::MalformedParams { cause, .. }
            | Self::ParamsNotSerializable { cause, .. } => Some(cause),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::test_support::{line, repository_path};
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

    /// テストが指す位置。行頭から数えた列で組み立てる。
    fn position_after(line_number: usize, preceding: &str) -> SourcePosition {
        SourcePosition::from_preceding_text(line(line_number), preceding)
    }

    /// hover の応答 1 通分の payload。
    fn hover_response(signature: &str) -> String {
        let contents = json!({
            "kind": "markdown",
            "value": format!("\n```typescript\n{signature}\n```\n"),
        });

        json!({"jsonrpc": "2.0", "id": 1, "result": {"contents": contents}}).to_string()
    }

    #[test]
    fn test_hover_returns_the_signature_the_server_answered() {
        let server_output = frames_of(&[&hover_response("function decl(a: string): number")]);
        let mut connection = connection_over(&server_output);
        let document = document_of("tests/fixtures/billing/discount.ts", "export const a = 1;");
        connection.open_document(&document).expect("送れる");

        let signature = connection
            .hover(&document, position_after(5, "export function "))
            .expect("応答を受け取れる");

        assert_eq!(
            signature,
            HoverOutcome::Answered("function decl(a: string): number".to_owned())
        );
    }

    #[test]
    fn test_hover_names_the_document_and_the_position_it_asks_about() {
        // 行は 0 始まりに直して送る。1 始まりのまま送ると 1 行下を見ることになる
        let server_output = frames_of(&[&hover_response("function decl(a: string): number")]);
        let mut connection = connection_over(&server_output);
        let document = document_of("tests/fixtures/billing/discount.ts", "export const a = 1;");
        connection.open_document(&document).expect("送れる");

        connection
            .hover(&document, position_after(5, "export function "))
            .expect("応答を受け取れる");

        let sent = sent_payloads(&connection.writer);
        let asked = sent
            .iter()
            .find(|payload| payload["method"] == json!("textDocument/hover"))
            .expect("hover を送っている");
        assert_eq!(
            asked["params"]["textDocument"]["uri"],
            json!(document.uri().as_str())
        );
        assert_eq!(
            asked["params"]["position"],
            json!({"line": 4, "character": 16})
        );
    }

    #[test]
    fn test_hover_where_the_server_has_no_answer_returns_nothing() {
        // 対照は上のテスト。同じ位置で、サーバが null を返す側
        let server_output = frames_of(&[r#"{"jsonrpc":"2.0","id":1,"result":null}"#]);
        let mut connection = connection_over(&server_output);
        let document = document_of("tests/fixtures/billing/discount.ts", "export const a = 1;");
        connection.open_document(&document).expect("送れる");

        let signature = connection
            .hover(&document, position_after(5, "export function "))
            .expect("応答を受け取れる");

        assert_eq!(signature, HoverOutcome::NoAnswer);
    }

    #[test]
    fn test_hover_on_a_document_that_was_not_opened_is_refused_before_it_is_sent() {
        // 送ってしまうとサーバは知らない URI に null を返し、「型が無い」と区別が付かない
        let mut connection = connection_over(&[]);
        let never_opened =
            document_of("tests/fixtures/inventory/reorder.ts", "export const b = 2;");

        let error = connection
            .hover(&never_opened, position_after(5, "export function "))
            .expect_err("送らずに断る");

        assert!(matches!(
            error,
            ConnectionError::DocumentNotOpen { uri } if uri == *never_opened.uri()
        ));
        assert!(
            sent_payloads(&connection.writer).is_empty(),
            "断ったので 1 通も送っていない"
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

    /// 参照元 1 件を返す応答。
    fn references_response(id: u64, uri: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"result":[{{"uri":"{uri}","range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":1}}}}}}]}}"#
        )
    }

    /// 参照元が 1 件も無いという応答。
    fn no_references_response(id: u64) -> String {
        format!(r#"{{"jsonrpc":"2.0","id":{id},"result":[]}}"#)
    }

    /// サーバが作業を始めるときに送ってくる要求。
    fn progress_create_request(id: u64, token: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"window/workDoneProgress/create","params":{{"token":"{token}"}}}}"#
        )
    }

    /// その作業が始まったという通知。
    fn progress_begin_notification(token: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"$/progress","params":{{"token":"{token}","value":{{"kind":"begin","title":"読み込み"}}}}}}"#
        )
    }

    /// その作業が終わったという通知。
    fn progress_end_notification(token: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"$/progress","params":{{"token":"{token}","value":{{"kind":"end"}}}}}}"#
        )
    }

    /// 参照元を尋ねられる状態のドキュメント。
    fn opened_document<R: BufRead>(connection: &mut Connection<R, Vec<u8>>) -> SourceDocument {
        let document = document_of("tests/fixtures/billing/discount.ts", "export const a = 1;");
        connection.open_document(&document).expect("送れる");
        document
    }

    #[test]
    fn test_references_returns_the_files_the_server_answered() {
        let server_output = frames_of(&[&references_response(
            1,
            "file:///repo/src/billing/invoice.ts",
        )]);
        let mut connection = connection_over(&server_output);
        let document = opened_document(&mut connection);

        let outcome = connection
            .references(&document, position_after(5, "export function "))
            .expect("応答を受け取れる");

        let ReferencesOutcome::Answered(paths) = outcome else {
            panic!("参照元が返る: {outcome:?}");
        };
        assert_eq!(paths, vec![PathBuf::from("/repo/src/billing/invoice.ts")]);
    }

    #[test]
    fn test_references_answered_while_the_server_is_still_working_asks_again_when_it_finished() {
        // サーバは読み込みの途中でも答えるが、まだ見ていないファイルの参照元は入らない。
        // 1 通目の空の答えをそのまま返すと、「呼び出し元が無い」と読める
        let server_output = frames_of(&[
            &progress_create_request(9, "loading"),
            &progress_begin_notification("loading"),
            &no_references_response(1),
            &progress_end_notification("loading"),
            &references_response(2, "file:///repo/src/billing/invoice.ts"),
        ]);
        let mut connection = connection_over(&server_output);
        let document = opened_document(&mut connection);

        let outcome = connection
            .references(&document, position_after(5, "export function "))
            .expect("応答を受け取れる");

        assert!(
            matches!(outcome, ReferencesOutcome::Answered(ref paths) if paths.len() == 1),
            "読み込みが終わってから尋ね直した答えを返す: {outcome:?}"
        );
    }

    #[test]
    fn test_references_answered_after_work_that_began_and_ended_asks_again() {
        // **作業が始まって終わるまでが、応答より先に届く**（実測で起きる）。
        // 「今動いているか」だけを見ていると、読み込み前に計算された答えを
        // 何も動いていない状態で受け取り、そのまま最終の答えにしてしまう
        let server_output = frames_of(&[
            &progress_create_request(9, "loading"),
            &progress_begin_notification("loading"),
            &progress_end_notification("loading"),
            &no_references_response(1),
            &references_response(2, "file:///repo/src/billing/invoice.ts"),
        ]);
        let mut connection = connection_over(&server_output);
        let document = opened_document(&mut connection);

        let outcome = connection
            .references(&document, position_after(5, "export function "))
            .expect("応答を受け取れる");

        assert!(
            matches!(outcome, ReferencesOutcome::Answered(ref paths) if paths.len() == 1),
            "作業に触れていない答えを返す: {outcome:?}"
        );
    }

    #[test]
    fn test_references_interrupted_again_keeps_asking_until_no_work_is_running() {
        // 2 通目を返している間に別の作業が始まる（monorepo が次のプロジェクトを読むなど）。
        // 2 通目をそのまま返すと、まだ見ていないファイルの分が抜けた答えが最終になる
        let server_output = frames_of(&[
            &progress_create_request(9, "loading-first"),
            &progress_begin_notification("loading-first"),
            &no_references_response(1),
            &progress_end_notification("loading-first"),
            &progress_create_request(10, "loading-next"),
            &progress_begin_notification("loading-next"),
            &no_references_response(2),
            &progress_end_notification("loading-next"),
            &references_response(3, "file:///repo/src/billing/invoice.ts"),
        ]);
        let mut connection = connection_over(&server_output);
        let document = opened_document(&mut connection);

        let outcome = connection
            .references(&document, position_after(5, "export function "))
            .expect("応答を受け取れる");

        assert!(
            matches!(outcome, ReferencesOutcome::Answered(ref paths) if paths.len() == 1),
            "作業が動いていないときの答えを返す: {outcome:?}"
        );
    }

    #[test]
    fn test_references_that_never_settles_does_not_return_a_partial_answer() {
        // 対照は上のテスト。最後の答えは 1 件持っているが、**その答えを返している間も
        // 作業が動いている**。途中の答えを最終的なシグナルとして返すと、呼び出し元の
        // 分布が実際より狭く出る
        let server_output = frames_of(&[
            &progress_create_request(9, "loading-first"),
            &progress_begin_notification("loading-first"),
            &no_references_response(1),
            &progress_end_notification("loading-first"),
            &progress_create_request(10, "loading-next"),
            &progress_begin_notification("loading-next"),
            &no_references_response(2),
            &progress_end_notification("loading-next"),
            &progress_create_request(11, "loading-more"),
            &progress_begin_notification("loading-more"),
            &references_response(3, "file:///repo/src/billing/invoice.ts"),
        ]);
        let mut connection = connection_over(&server_output);
        let document = opened_document(&mut connection);

        let outcome = connection
            .references(&document, position_after(5, "export function "))
            .expect("応答を受け取れる");

        assert!(
            matches!(outcome, ReferencesOutcome::ServerStillWorking),
            "落ち着かなかったことを名前で返す: {outcome:?}"
        );
    }

    #[test]
    fn test_references_answered_while_work_prepared_earlier_ran_asks_again() {
        // token を用意するのは前の問い合わせ（hover）の最中で、**始まりと終わりだけが
        // 参照元を尋ねている間に届く**。用意した時点しか数えていないと、この並びで
        // 「何も起きなかった」ように見え、読み込み中の答えをそのまま採る
        let server_output = frames_of(&[
            &progress_create_request(9, "loading"),
            &hover_response("function applyDiscount(invoice: Invoice): number"),
            &progress_begin_notification("loading"),
            &progress_end_notification("loading"),
            &no_references_response(2),
            &references_response(3, "file:///repo/src/billing/invoice.ts"),
        ]);
        let mut connection = connection_over(&server_output);
        let document = opened_document(&mut connection);
        connection
            .hover(&document, position_after(5, "export function "))
            .expect("hover に答えが返る");

        let outcome = connection
            .references(&document, position_after(5, "export function "))
            .expect("応答を受け取れる");

        assert!(
            matches!(outcome, ReferencesOutcome::Answered(ref paths) if paths.len() == 1),
            "前の問い合わせで用意された作業が動いたなら尋ね直す: {outcome:?}"
        );
    }

    #[test]
    fn test_references_with_a_progress_that_never_begins_does_not_wait_for_it() {
        // token を作ってよいかの要求は、作業が始まったことを意味しない。始まりの通知が
        // 来ないまま捨てられた token を待つと、**終わりの来ない作業を待ち続ける**
        let server_output = frames_of(&[
            &progress_create_request(9, "abandoned"),
            &no_references_response(1),
            &references_response(2, "file:///repo/src/billing/invoice.ts"),
        ]);
        let mut connection = connection_over(&server_output);
        let document = opened_document(&mut connection);

        let outcome = connection
            .references(&document, position_after(5, "export function "))
            .expect("応答を受け取れる");

        assert!(
            matches!(outcome, ReferencesOutcome::Answered(ref paths) if paths.len() == 1),
            "始まっていない作業は待たずに尋ね直す: {outcome:?}"
        );
    }

    #[test]
    fn test_references_answered_while_nothing_is_running_asks_only_once() {
        // 対照は上のテスト。作業が始まっていなければ待つものが無いので、1 往復で終わる。
        // 待ってしまうと、何も送ってこないサーバの前で止まる
        let server_output = frames_of(&[&no_references_response(1)]);
        let mut connection = connection_over(&server_output);
        let document = opened_document(&mut connection);

        connection
            .references(&document, position_after(5, "export function "))
            .expect("応答を受け取れる");

        let asked = sent_methods(&connection.writer)
            .iter()
            .filter(|method| *method == References::METHOD)
            .count();
        assert_eq!(asked, 1);
    }

    #[test]
    fn test_references_accepts_the_progress_the_server_asks_to_create() {
        // 断る（method not found）と、サーバは進捗を送ってこない。送ってこなければ
        // 読み込み中かどうかが分からず、尋ね直す機会も無くなる
        let server_output = frames_of(&[
            &progress_create_request(9, "loading"),
            &progress_begin_notification("loading"),
            &no_references_response(1),
            &progress_end_notification("loading"),
            &no_references_response(2),
        ]);
        let mut connection = connection_over(&server_output);
        let document = opened_document(&mut connection);

        connection
            .references(&document, position_after(5, "export function "))
            .expect("応答を受け取れる");

        let sent = sent_payloads(&connection.writer);
        let answered = sent
            .iter()
            .find(|payload| payload["id"] == json!(9))
            .expect("進捗の要求に返している");
        assert_eq!(answered["result"], Value::Null);
    }

    #[test]
    fn test_references_with_a_progress_notification_it_cannot_read_reports_it() {
        // 読めない進捗を黙って捨てると、終わりの来ない作業を待ち続ける
        let server_output = frames_of(&[
            &progress_create_request(9, "loading"),
            r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"loading"}}"#,
        ]);
        let mut connection = connection_over(&server_output);
        let document = opened_document(&mut connection);

        let error = connection
            .references(&document, position_after(5, "export function "))
            .expect_err("読み進められない");

        assert!(matches!(error, ConnectionError::MalformedParams { .. }));
    }
}
