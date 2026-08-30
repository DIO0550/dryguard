//! LSP サーバとの会話。子プロセスの起動と JSON-RPC に閉じる。
//!
//! 複数のステージを組み合わせる手順はここには置かない
//! (rules/architecture.md「依存方向のルール」)。
//!
//! | モジュール | 持つもの |
//! |---|---|
//! | `framing` | `Content-Length` による区切り |
//! | `message` | JSON-RPC の payload の組み立てと解釈 |
//! | `connection` | 要求と応答の対応付け・ライフサイクル |
//! | `uri` | パスから `file:` URI への変換 |
//! | `workspace` | サーバに見せるワークスペースの根 |
//! | `document` | サーバに開かせるソースファイル |
//! | `hover` | hover の応答から型の綴りを取り出す |
//! | `references` | references の応答から参照元のファイルを取り出す |
//! | ここ | サーバの起動・パイプの配線・終了 |
//!
//! **外へ出すのは [`ServerCommand`] / [`Client`] / [`Session`]、渡す値
//! （[`WorkspaceRoot`] / [`SourceDocument`]）と、失敗を読むための型だけ。**
//! 区切りや payload の組み立て方は、いつ変えても外に影響しない位置に置く
//! (rules/architecture.md「モジュールの公開 API」)。

pub(crate) mod connection;
pub(crate) mod document;
pub(crate) mod framing;
pub(crate) mod hover;
pub(crate) mod message;
pub(crate) mod references;
pub(crate) mod uri;
pub(crate) mod workspace;

use std::error::Error;
use std::fmt;
use std::io::{self, BufReader};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};

use lsp_types::{HoverProviderCapability, OneOf, ServerCapabilities};

use crate::source_position::SourcePosition;
use connection::Connection;

// 開かせるドキュメントとワークスペースの根は、呼ぶ側が組み立てて渡す。
pub use document::{DocumentError, SourceDocument};
// hover / references の結果は「取れた / 取れなかった理由」を分けて持つので、
// 外から読める形で出す。
pub use hover::HoverOutcome;
pub use references::ReferencesOutcome;
pub use workspace::{WorkspaceError, WorkspaceRoot};

// 失敗を読むための型だけを外へ出す。[`ClientError`] が抱えている以上、
// 外から名前を呼べないと `source()` をたどっても中身を見分けられない。
pub use connection::ConnectionError;
pub use framing::FramingError;
pub use message::{MessageError, RequestId, ResponseFailure};
pub use uri::{PathUriError, UriPathError};

/// TypeScript の LSP サーバの実行ファイル名。
const TYPESCRIPT_SERVER: &str = "typescript-language-server";

/// stdio でしゃべらせる指定。付けないとサーバは使い方を表示して終わる。
const STDIO_OPTION: &str = "--stdio";

/// どの LSP サーバをどう起動するか。
///
/// **サーバごとの差はここに閉じる。** Phase 4 で rust-analyzer を挿すときに
/// 足すのはこの値で、[`Client`] 側は変わらない
/// (`docs/dryguard-plan.md`「LSPサーバ: TS は typescript-language-server、
/// Rust は rust-analyzer」)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCommand {
    program: String,
    args: Vec<String>,
}

impl ServerCommand {
    /// 実行ファイル名と引数から起動の仕方を組み立てる。
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
        }
    }

    /// typescript-language-server を stdio で起動する指定。
    pub fn typescript() -> Self {
        Self::new(TYPESCRIPT_SERVER, vec![STDIO_OPTION.to_owned()])
    }

    /// 実行ファイル名。
    pub fn program(&self) -> &str {
        &self.program
    }
}

/// 起動しただけの LSP サーバ。まだ握手していない。
///
/// **握手の前と後を別の型にしてある。** LSP は `initialize` を 1 回しか受け付けず、
/// その前に他の要求を送ることも許さない。1 つの型に両方の状態を持たせると、
/// 2 回目の握手も、握手前の終了も、書けてしまう
/// (rules/coding.md「不正な状態を型で表現できなくする」)。
///
/// 子プロセスを抱えるので、[`Session::shutdown`] を通らずに落ちた経路では
/// `Drop` が kill する。
#[derive(Debug)]
pub struct Client {
    child: Child,
    connection: Connection<BufReader<ChildStdout>, ChildStdin>,
    program: String,
    terminated: bool,
}

impl Client {
    /// サーバを起動し、stdin / stdout を配線する。
    ///
    /// stderr は捨てる。サーバのログをこちらの出力に混ぜないため。起動直後に黙った場合は
    /// [`ClientError::ServerClosedDuringHandshake`] として表に出る。
    ///
    /// **Why not（`Stdio::piped()` で診断を取る）**: こちらが読まないままにすると、
    /// パイプが埋まった時点でサーバが write でブロックする。診断が消えるより悪い。
    ///
    /// # Errors
    ///
    /// 実行ファイルが見つからないとき、起動できないとき、パイプを取り出せないとき。
    pub fn start(command: &ServerCommand) -> Result<Self, ClientError> {
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|cause| spawn_error_of(&command.program, cause))?;

        let pipes = child.stdin.take().zip(child.stdout.take());
        let Some((stdin, stdout)) = pipes else {
            // piped を指定した以上ここへは来ないが、来たときに子プロセスを残さない。
            let _ = child.kill();
            return Err(ClientError::PipesNotWired);
        };

        Ok(Self {
            child,
            connection: Connection::new(BufReader::new(stdout), stdin),
            program: command.program.clone(),
            terminated: false,
        })
    }

    /// サーバと握手し、問い合わせを送れる状態にする。
    ///
    /// `root` はサーバに見せるワークスペースの根。開くファイルを含む位置を渡す。
    ///
    /// 値を取るのは、握手を 2 回できないようにするため。`initialize` を 2 度送られた
    /// サーバは 2 通目を拒む。
    ///
    /// # Errors
    ///
    /// 往復が失敗したとき。サーバが答えないまま出力を閉じた場合は
    /// [`ClientError::ServerClosedDuringHandshake`]。抜けた [`Client`] は `Drop` が kill する。
    pub fn handshake(mut self, root: &WorkspaceRoot) -> Result<Session, ClientError> {
        let capabilities = match self.connection.handshake(root) {
            Ok(capabilities) => capabilities,
            Err(cause) => return Err(handshake_error_of(&self.program, cause)),
        };

        Ok(Session {
            client: self,
            capabilities,
        })
    }
}

/// 握手を終えた LSP サーバ。問い合わせを送れる。
///
/// 握手で受け取った capabilities を抱えるのは、**サーバができることを知らずに
/// 問い合わせを組み立てる形を作らない**ため。
#[derive(Debug)]
pub struct Session {
    client: Client,
    capabilities: ServerCapabilities,
}

impl Session {
    /// サーバができること。
    pub fn capabilities(&self) -> &ServerCapabilities {
        &self.capabilities
    }

    /// 候補ペアのファイルをサーバに開かせる。
    ///
    /// **開くのは候補ペアが含まれるファイルだけ。** コードベース全体を開かせると
    /// rust-analyzer が実用にならない（`docs/dryguard-plan.md`「Stage 2: 意味情報収集」）。
    ///
    /// # Errors
    ///
    /// 送信が失敗したとき。
    pub fn open_document(&mut self, document: &SourceDocument) -> Result<(), ClientError> {
        self.client
            .connection
            .open_document(document)
            .map_err(ClientError::Conversation)
    }

    /// 開かせたファイルの、指定位置にある名前の型の綴りを尋ねる。
    ///
    /// `position` は `Chunk::name_position` が指す識別子の位置。答えが返ったのか、
    /// 無かったのか、読めなかったのかは [`HoverOutcome`] が分けて持つ。
    ///
    /// **hover を提供していないサーバには送らない**（[`HoverOutcome::NotSupported`]）。
    /// 握手で受け取った capabilities を抱えているのはこのためで、送ってしまうと
    /// 仕様に忠実なサーバは `MethodNotFound` を返し、**シグナルが取れないだけの話が
    /// 往復の失敗になる**。
    ///
    /// 先に [`Session::open_document`] で開かせておく。開かせていないドキュメントへは
    /// 送らずに断る（サーバは知らない URI に null を返すので、「型が無い」と
    /// 区別が付かなくなる）。
    ///
    /// # Errors
    ///
    /// そのドキュメントを開かせていないとき、往復が失敗したとき、
    /// 応答を hover の結果として読めないとき。
    pub fn hover(
        &mut self,
        document: &SourceDocument,
        position: SourcePosition,
    ) -> Result<HoverOutcome, ClientError> {
        if !provides_hover(&self.capabilities) {
            return Ok(HoverOutcome::NotSupported);
        }

        self.client
            .connection
            .hover(document, position)
            .map_err(ClientError::Conversation)
    }

    /// 開かせたファイルの、指定位置にある名前を参照しているところを尋ねる。
    ///
    /// `position` は `Chunk::name_position` が指す識別子の位置。参照元が返ったのか、
    /// 無かったのか、読めなかったのかは [`ReferencesOutcome`] が分けて持つ。
    ///
    /// **references を提供していないサーバには送らない**（[`ReferencesOutcome::NotSupported`]）。
    /// hover と同じ理由で、送ると**シグナルが取れないだけの話が往復の失敗になる**。
    ///
    /// 先に [`Session::open_document`] で開かせておく。
    ///
    /// # Errors
    ///
    /// そのドキュメントを開かせていないとき、往復が失敗したとき、
    /// 応答を references の結果として読めないとき。
    pub fn references(
        &mut self,
        document: &SourceDocument,
        position: SourcePosition,
    ) -> Result<ReferencesOutcome, ClientError> {
        if !provides_references(&self.capabilities) {
            return Ok(ReferencesOutcome::NotSupported);
        }

        self.client
            .connection
            .references(document, position)
            .map_err(ClientError::Conversation)
    }

    /// 開かせたファイルを閉じさせる。開いていなければ何もしない。
    ///
    /// # Errors
    ///
    /// 送信が失敗したとき。
    pub fn close_document(&mut self, document: &SourceDocument) -> Result<(), ClientError> {
        self.client
            .connection
            .close_document(document)
            .map_err(ClientError::Conversation)
    }

    /// サーバを終わらせ、子プロセスの終了を待つ。
    ///
    /// 値を取るのは、終わらせた後の [`Session`] を残さないため
    /// (rules/coding.md「不正な状態を型で表現できなくする」)。
    ///
    /// # Errors
    ///
    /// 往復が失敗したとき、終了を待てなかったとき、サーバが異常終了したとき。
    /// 往復の失敗で抜けた場合は `Drop` が kill する。
    pub fn shutdown(mut self) -> Result<(), ClientError> {
        self.client
            .connection
            .shutdown()
            .map_err(ClientError::Conversation)?;
        let status = self.client.child.wait().map_err(ClientError::Wait)?;

        // 待ち終えた時点で子プロセスは残っていない。ここより後で失敗しても kill は要らない。
        self.client.terminated = true;

        // `wait` は終了できたことしか言わない。**異常終了も `Ok` で返る**ので、
        // 状態を見ずに握りつぶすと「終了しました」と報告してしまう
        // (rules/coding.md「失敗を握りつぶして既定値へフォールバックしない」)。
        if !status.success() {
            return Err(ClientError::AbnormalExit { status });
        }

        Ok(())
    }
}

impl Drop for Client {
    /// 終了手順を通らずに落ちた経路で、子プロセスを残さない。
    ///
    /// 失敗しても報告先が無いので捨てる。ここで報告できないことが、
    /// [`Client::shutdown`] を別に持つ理由でもある。
    fn drop(&mut self) {
        if self.terminated {
            return;
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// そのサーバが hover に答えるか。
///
/// **有無ではなく中身を見る。** 無効を表す `Simple(false)` も「宣言はある」ので、
/// `is_some()` で見ると hover を切ったサーバへ送ってしまう。
fn provides_hover(capabilities: &ServerCapabilities) -> bool {
    matches!(
        capabilities.hover_provider,
        Some(HoverProviderCapability::Simple(true) | HoverProviderCapability::Options(_))
    )
}

/// そのサーバが references に答えるか。
///
/// hover と同じく**有無ではなく中身を見る**。無効を表す `Left(false)` も「宣言はある」ので、
/// `is_some()` で見ると references を切ったサーバへ送ってしまう。
fn provides_references(capabilities: &ServerCapabilities) -> bool {
    matches!(
        capabilities.references_provider,
        Some(OneOf::Left(true) | OneOf::Right(_))
    )
}

/// 握手の失敗を、サーバが黙った場合とそれ以外に分ける。
///
/// フレームの切れ目の EOF は「起動はしたが、答えないまま出力を閉じた」。stderr を捨てて
/// いるので**閉じた理由そのものは残っていない**が、利用者が次に試すこと（サーバを直接
/// 起動して起動時のエラーを見る）は他の失敗と違うので、専用のバリアントで返す。
///
/// **Why not（`Child::try_wait` で終了を確かめてから名乗る）**: EOF の直後は、
/// 終了したサーバでもまだ回収できていないことがある。確かめたつもりで取り違えるより、
/// **観測した事実（出力が閉じた）だけを名前にする**
/// (rules/naming.md「名前と実体を一致させる」)。
fn handshake_error_of(program: &str, cause: ConnectionError) -> ClientError {
    if matches!(cause, ConnectionError::Framing(FramingError::ServerClosed)) {
        return ClientError::ServerClosedDuringHandshake {
            program: program.to_owned(),
        };
    }

    ClientError::Conversation(cause)
}

/// 起動の失敗を、実行ファイルが無い場合とそれ以外に分ける。
///
/// 「入っていないので入れてください」と「起動できたが駄目だった」では、
/// 利用者が直す先が違う。
fn spawn_error_of(program: &str, cause: io::Error) -> ClientError {
    if cause.kind() == io::ErrorKind::NotFound {
        return ClientError::ServerNotFound {
            program: program.to_owned(),
        };
    }

    ClientError::Spawn {
        program: program.to_owned(),
        cause,
    }
}

/// LSP サーバとのやりとりが失敗した理由。
#[derive(Debug)]
pub enum ClientError {
    /// 実行ファイルが見つからない。
    ServerNotFound {
        /// 見つからなかった実行ファイル名。
        program: String,
    },
    /// 起動できなかった。
    Spawn {
        /// 起動しようとした実行ファイル名。
        program: String,
        /// 起動できなかった理由。
        cause: io::Error,
    },
    /// 子プロセスの stdin / stdout を取り出せなかった。
    PipesNotWired,
    /// 起動はしたが、握手に答えないまま出力を閉じた。
    ///
    /// 子プロセスが終了したかまでは見ていない。閉じた時点で会話は続けられないので、
    /// どちらでも `Drop` が kill する。
    ServerClosedDuringHandshake {
        /// 黙った実行ファイル名。
        program: String,
    },
    /// 起動した後の往復が失敗した。
    Conversation(ConnectionError),
    /// 子プロセスの終了を待てなかった。
    Wait(io::Error),
    /// 終了手順は通ったが、サーバが異常終了した。
    AbnormalExit {
        /// サーバの終了状態。
        status: ExitStatus,
    },
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ServerNotFound { program } => {
                write!(formatter, "LSP サーバが見つかりません: {program}")
            }
            Self::Spawn { program, cause } => {
                write!(formatter, "LSP サーバを起動できません ({program}): {cause}")
            }
            Self::PipesNotWired => write!(
                formatter,
                "LSP サーバの stdin / stdout を取り出せませんでした"
            ),
            // 閉じた理由はこちらに残っていない。次に試すことを出す。
            Self::ServerClosedDuringHandshake { program } => write!(
                formatter,
                "LSP サーバ ({program}) が握手に答えないまま出力を閉じました。\
                 {program} を直接起動して、起動時のエラーを確認してください"
            ),
            Self::Conversation(cause) => write!(formatter, "{cause}"),
            Self::Wait(cause) => {
                write!(formatter, "LSP サーバの終了を待てませんでした: {cause}")
            }
            Self::AbnormalExit { status } => {
                write!(formatter, "LSP サーバが異常終了しました ({status})")
            }
        }
    }
}

impl Error for ClientError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ServerNotFound { .. }
            | Self::PipesNotWired
            | Self::ServerClosedDuringHandshake { .. }
            | Self::AbnormalExit { .. } => None,
            Self::Spawn { cause, .. } => Some(cause),
            Self::Conversation(cause) => Some(cause),
            Self::Wait(cause) => Some(cause),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use crate::codebase;
    use crate::test_support::{line, repository_path};
    use lsp_types::HoverProviderCapability;

    /// 候補ペアのファイルとして開かせる fixture。
    const A_CANDIDATE_PAIR_FILE: &str = "tests/fixtures/billing/discount.ts";

    fn fixture_workspace_root() -> WorkspaceRoot {
        WorkspaceRoot::enclosing(&[repository_path(A_CANDIDATE_PAIR_FILE)]).expect("根を決められる")
    }

    fn fixture_document() -> SourceDocument {
        let path = repository_path(A_CANDIDATE_PAIR_FILE);
        let text = codebase::source_of(&path).expect("fixture を読める");

        SourceDocument::new(&path, text).expect("ドキュメントにできる")
    }

    /// 呼び出し元を持つ fixture のうち、`applyDiscount` を宣言しているファイル。
    ///
    /// `tests/fixtures/references/` は tsconfig.json を持つ木にしてある。**参照元は
    /// 開かせたファイルからは辿れない**（呼び出し元が呼び出し先を import するので、
    /// import を辿る向きが逆）ため、サーバがコードベース全体をプロジェクトとして
    /// 見ていないと 1 件も返らない。
    const A_CALLED_FILE: &str = "tests/fixtures/references/src/billing/discount.ts";

    /// 誰にも呼ばれていない関数を持つ fixture。
    const AN_UNCALLED_FILE: &str = "tests/fixtures/references/src/report/monthly.ts";

    /// その木のプロジェクト設定。
    ///
    /// **根はここを含む位置にする。** tsconfig.json より下を根にすると、サーバは
    /// 開いたファイルとその import 先だけのプロジェクトを組み立て、**呼び出し元の
    /// 一部しか返さない**（呼び出し元は import を辿る向きの逆にある）。
    const THE_PROJECT_FILE: &str = "tests/fixtures/references/tsconfig.json";

    fn references_fixture_root() -> WorkspaceRoot {
        WorkspaceRoot::enclosing(&[
            repository_path(A_CALLED_FILE),
            repository_path(THE_PROJECT_FILE),
        ])
        .expect("根を決められる")
    }

    fn document_of(relative_path: &str) -> SourceDocument {
        let path = repository_path(relative_path);
        let text = codebase::source_of(&path).expect("fixture を読める");

        SourceDocument::new(&path, text).expect("ドキュメントにできる")
    }

    /// 開かせたファイルの、その位置にある名前の参照元をサーバに尋ねる。
    fn references_at(relative_path: &str, position: SourcePosition) -> ReferencesOutcome {
        let client = Client::start(&ServerCommand::typescript()).expect("サーバを起動できる");
        let mut session = client
            .handshake(&references_fixture_root())
            .expect("握手できる");
        let document = document_of(relative_path);
        session.open_document(&document).expect("開かせられる");

        let outcome = session
            .references(&document, position)
            .expect("問い合わせられる");

        session.shutdown().expect("終了できる");
        outcome
    }

    #[test]
    fn test_client_start_with_a_missing_program_reports_the_server_not_found() {
        let command = ServerCommand::new("dryguard-no-such-language-server", Vec::new());

        let error = Client::start(&command).expect_err("起動できない");

        assert!(matches!(
            error,
            ClientError::ServerNotFound { program } if program == "dryguard-no-such-language-server"
        ));
    }

    /// そのサーバができることとして hover だけを宣言した capabilities。
    fn capabilities_declaring_hover(
        hover_provider: Option<HoverProviderCapability>,
    ) -> ServerCapabilities {
        ServerCapabilities {
            hover_provider,
            ..ServerCapabilities::default()
        }
    }

    #[test]
    fn test_provides_hover_with_a_server_that_declares_it_is_true() {
        let capabilities =
            capabilities_declaring_hover(Some(HoverProviderCapability::Simple(true)));

        assert!(provides_hover(&capabilities));
    }

    #[test]
    fn test_provides_hover_with_a_server_that_turned_it_off_is_false() {
        // 対照は上のテスト。**宣言はあるが無効**という形で、`is_some()` で見ていると
        // hover を切ったサーバへ要求を送ってしまう
        let capabilities =
            capabilities_declaring_hover(Some(HoverProviderCapability::Simple(false)));

        assert!(!provides_hover(&capabilities));
    }

    #[test]
    fn test_provides_hover_with_a_server_that_does_not_declare_it_is_false() {
        let capabilities = capabilities_declaring_hover(None);

        assert!(!provides_hover(&capabilities));
    }

    /// そのサーバができることとして references だけを宣言した capabilities。
    fn capabilities_declaring_references(
        references_provider: Option<OneOf<bool, lsp_types::ReferencesOptions>>,
    ) -> ServerCapabilities {
        ServerCapabilities {
            references_provider,
            ..ServerCapabilities::default()
        }
    }

    #[test]
    fn test_provides_references_with_a_server_that_declares_it_is_true() {
        let capabilities = capabilities_declaring_references(Some(OneOf::Left(true)));

        assert!(provides_references(&capabilities));
    }

    #[test]
    fn test_provides_references_with_a_server_that_turned_it_off_is_false() {
        // 対照は上のテスト。**宣言はあるが無効**という形で、`is_some()` で見ていると
        // references を切ったサーバへ要求を送ってしまう
        let capabilities = capabilities_declaring_references(Some(OneOf::Left(false)));

        assert!(!provides_references(&capabilities));
    }

    #[test]
    fn test_provides_references_with_a_server_that_does_not_declare_it_is_false() {
        let capabilities = capabilities_declaring_references(None);

        assert!(!provides_references(&capabilities));
    }

    #[test]
    fn test_handshake_error_of_a_server_that_closed_names_the_program() {
        // 起動して答えないまま出力を閉じたサーバ。理由は stderr と共に消えているので、
        // 利用者が次に試すこと（直接起動して確かめる）を出せる形で返す
        let cause = ConnectionError::Framing(FramingError::ServerClosed);

        let error = handshake_error_of("typescript-language-server", cause);

        assert!(matches!(
            error,
            ClientError::ServerClosedDuringHandshake { program }
                if program == "typescript-language-server"
        ));
    }

    #[test]
    fn test_handshake_error_of_another_framing_failure_stays_a_conversation_error() {
        // 「サーバが黙った」以外まで起動失敗として畳むと、直す先を取り違える
        let cause = ConnectionError::Framing(FramingError::MissingContentLength);

        let error = handshake_error_of("typescript-language-server", cause);

        assert!(matches!(error, ClientError::Conversation(_)));
    }

    #[test]
    fn test_server_command_for_typescript_speaks_over_stdio() {
        // --stdio が無いとサーバは使い方を表示して終わり、応答は 1 つも返らない
        let command = ServerCommand::typescript();

        assert_eq!(command.program(), "typescript-language-server");
        assert_eq!(command.args, vec!["--stdio".to_owned()]);
    }

    #[test]
    #[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
    fn test_client_handshake_with_typescript_language_server_returns_its_capabilities() {
        let command = ServerCommand::typescript();
        let client = Client::start(&command).expect("サーバを起動できる");

        let session = client
            .handshake(&fixture_workspace_root())
            .expect("握手できる");

        // hover は Stage 2 で最初に使う問い合わせ。返らないサーバでは意味情報が採れない
        assert!(
            provides_hover(session.capabilities()),
            "typescript-language-server は hover を提供する"
        );

        session.shutdown().expect("終了できる");
    }

    #[test]
    #[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
    fn test_session_still_answers_after_opening_and_closing_a_candidate_pair_file() {
        // `didOpen` / `didClose` は通知なので応答が返らない。**受け取れたかは次の要求で分かる**
        // ので、開いて閉じた後の終了手順（`shutdown` 要求の往復と正常終了）で確かめる。
        // 開いたファイルの中身をサーバが読めているかは、hover を足す回に見る
        let command = ServerCommand::typescript();
        let client = Client::start(&command).expect("サーバを起動できる");
        let mut session = client
            .handshake(&fixture_workspace_root())
            .expect("握手できる");
        let document = fixture_document();

        session.open_document(&document).expect("開かせられる");
        session.close_document(&document).expect("閉じさせられる");

        session.shutdown().expect("終了できる");
    }

    #[test]
    #[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
    fn test_session_hover_on_a_function_name_answers_its_type_signature() {
        // fixture の 5 行目 `export function applyDiscount(invoice: Invoice): number`。
        // 開かせた中身をサーバが読めているかは、ここで初めて確かめられる
        let command = ServerCommand::typescript();
        let client = Client::start(&command).expect("サーバを起動できる");
        let mut session = client
            .handshake(&fixture_workspace_root())
            .expect("握手できる");
        let document = fixture_document();
        session.open_document(&document).expect("開かせられる");

        let signature = session
            .hover(
                &document,
                SourcePosition::from_preceding_text(line(5), "export function "),
            )
            .expect("問い合わせられる");

        assert_eq!(
            signature,
            HoverOutcome::Answered("function applyDiscount(invoice: Invoice): number".to_owned())
        );

        session.shutdown().expect("終了できる");
    }

    #[test]
    #[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
    fn test_session_hover_away_from_a_name_has_no_answer() {
        // 対照は上のテスト。同じファイルの同じ行で、識別子ではない位置（行頭の
        // `export` の手前）を指す。**位置がずれると黙って答えが消える**ので、
        // 「答えが返る位置」と「返らない位置」を両方見て初めて指し方を確かめられる
        let command = ServerCommand::typescript();
        let client = Client::start(&command).expect("サーバを起動できる");
        let mut session = client
            .handshake(&fixture_workspace_root())
            .expect("握手できる");
        let document = fixture_document();
        session.open_document(&document).expect("開かせられる");

        let signature = session
            .hover(&document, SourcePosition::from_preceding_text(line(5), ""))
            .expect("問い合わせられる");

        assert_eq!(signature, HoverOutcome::NoAnswer);

        session.shutdown().expect("終了できる");
    }

    #[test]
    #[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
    fn test_session_references_at_a_function_name_answers_the_files_that_call_it() {
        // fixture の 5 行目 `export function applyDiscount`。呼び出し元は同じ billing の
        // invoice.ts と statement.ts で、**どちらも開かせていない**（開かせるのは
        // 候補ペアのファイルだけ）
        let outcome = references_at(
            A_CALLED_FILE,
            SourcePosition::from_preceding_text(line(5), "export function "),
        );

        let ReferencesOutcome::Answered(paths) = outcome else {
            panic!("呼び出し元のあるチャンクには参照元が返る: {outcome:?}");
        };
        let names: BTreeSet<String> = paths
            .iter()
            .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
            .collect();
        assert_eq!(
            names,
            BTreeSet::from(["invoice.ts".to_owned(), "statement.ts".to_owned()])
        );
    }

    #[test]
    #[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
    fn test_session_references_at_a_function_nobody_calls_has_no_answer() {
        // 対照は上のテスト。同じ木の中で、誰にも呼ばれていない関数を指す。
        // 宣言そのものを数えていれば、ここでも 1 件返ってしまう
        let outcome = references_at(
            AN_UNCALLED_FILE,
            SourcePosition::from_preceding_text(line(4), "export function "),
        );

        assert!(
            matches!(outcome, ReferencesOutcome::NoAnswer),
            "呼び出し元の無いチャンクでは参照元が返らない: {outcome:?}"
        );
    }
}
