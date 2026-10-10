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
//! | `child_output` | 子プロセスの出力を吸い続け、期限付きで受け取る |
//! | `child_input` | 子プロセスの入力へ別スレッドで書き、期限付きで書き終わりを待つ |
//! | `uri` | パスから `file:` URI への変換 |
//! | `workspace` | サーバに見せるワークスペースの根 |
//! | `document` | サーバに開かせるソースファイル |
//! | `hover` | hover の応答から型の綴りを取り出す |
//! | `declaration_site` | typeDefinition / definition の応答から型の宣言の場所を取り出す |
//! | `references` | references の応答から参照元のファイルを取り出す |
//! | `call_hierarchy` | callHierarchy の応答から呼び出し先のファイルを取り出す |
//! | `project_membership` | projectInfo の応答を読み、設定されたプロジェクトか見分ける |
//! | ここ | サーバの起動・パイプの配線・終了 |
//!
//! **外へ出すのは [`ServerCommand`] / [`Client`] / [`Session`]、渡す値
//! （[`WorkspaceRoot`] / [`SourceDocument`]）と、失敗を読むための型だけ。**
//! 区切りや payload の組み立て方は、いつ変えても外に影響しない位置に置く
//! (rules/architecture.md「モジュールの公開 API」)。

pub(crate) mod call_hierarchy;
mod child_input;
mod child_output;
pub(crate) mod connection;
pub(crate) mod declaration_site;
pub(crate) mod document;
pub(crate) mod framing;
pub(crate) mod hover;
pub(crate) mod message;
pub(crate) mod project_membership;
pub(crate) mod references;
pub(crate) mod uri;
pub(crate) mod workspace;

use std::error::Error;
use std::fmt;
use std::io::{self, BufReader};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use lsp_types::{
    CallHierarchyServerCapability, HoverProviderCapability, ImplementationProviderCapability,
    OneOf, ServerCapabilities, TypeDefinitionProviderCapability,
};

use crate::source_position::SourcePosition;
use crate::syntax::tree::Grammar;
use child_input::IntakeLimitedWriter;
use child_output::{SilenceLimitedReader, StderrTail};
use connection::Connection;

// 開かせるドキュメントとワークスペースの根は、呼ぶ側が組み立てて渡す。
pub use document::{DocumentError, SourceDocument};
// hover / references / callHierarchy の結果は「取れた / 取れなかった理由」を分けて持つので、
// 外から読める形で出す。
pub use call_hierarchy::CalleesOutcome;
pub use hover::{HoverOutcome, SignatureText};
pub use references::{Reference, ReferencesOutcome};
// 型の宣言の場所は、開かせる相手を決める材料として `pipeline` が読む。
pub(crate) use declaration_site::UniqueDeclarationSiteOutcome;
pub use declaration_site::{DeclarationSite, DeclarationSiteOutcome, ImplementationOutcome};
pub use workspace::{WorkspaceError, WorkspaceRoot};
// 根の決め方と所属の確かめ方は `pipeline` だけが使う手順なので、クレートの外へは出さない
// (rules/architecture.md「モジュールの公開 API」)。所属のほうは**サーバ固有の要求の形**
// でもあるので、外へ出すと typescript-language-server の都合が公開 API に居座る。
pub(crate) use project_membership::ProjectMembershipOutcome;
pub(crate) use workspace::ProjectRoot;

// 失敗を読むための型だけを外へ出す。[`ClientError`] が抱えている以上、
// 外から名前を呼べないと `source()` をたどっても中身を見分けられない。
pub use connection::ConnectionError;
pub use framing::FramingError;
pub use message::{MessageError, RequestId, ResponseFailure};
pub use uri::{PathUriError, UriPathError};

/// TypeScript の LSP サーバの実行ファイル名。
const TYPESCRIPT_SERVER: &str = "typescript-language-server";

/// Rust の LSP サーバの実行ファイル名。
const RUST_SERVER: &str = "rust-analyzer";

/// stdio でしゃべらせる指定。付けないとサーバは使い方を表示して終わる。
const STDIO_OPTION: &str = "--stdio";

/// TypeScript のプロジェクトの範囲を決めるファイル。
///
/// tsserver はこれを見つけたディレクトリをプロジェクトの根にする。見つからなければ
/// 開いたファイルとその import 先だけを組み立てる（inferred project）ので、
/// **import を辿る向きの逆にある参照元が返らない**。
const TYPESCRIPT_PROJECT_MARKERS: [&str; 2] = ["tsconfig.json", "jsconfig.json"];

/// rust-analyzer がワークスペースを見つけるためのマニフェスト。
const RUST_PROJECT_MARKERS: [&str; 1] = ["Cargo.toml"];

/// サーバが何も送ってこないまま待つ上限。
///
/// rust-analyzer（2026-09-21 リリース）で dryguard 自身を初回に読ませたときの、
/// フレームの間の最長の沈黙が 6.69 秒（typescript-language-server は 0.63 秒）。
/// 大きなワークスペースの build script が報告なしに長引く分を見込んで、広く取る。
/// **狭く取ると、正常に遅いサーバを殺す側に倒れる。**
const SILENCE_LIMIT: Duration = Duration::from_secs(120);

/// こちらが送ったものを、サーバが受け取らないまま待つ上限。書き込みの 1 塊ごとに数える。
///
/// **沈黙の上限と揃える。** 狭くする根拠になる実測が無い。サーバが stdin をどう読むか
/// （読み取り専用のスレッドか、イベントループか）は確かめておらず、後者なら重い処理の間は
/// 受け取らないので、狭く取ると正常に遅いサーバを殺す側に倒れうる。
const INTAKE_LIMIT: Duration = SILENCE_LIMIT;

/// `exit` を送ってから、子プロセスが終わるのを待つ上限。
///
/// 同じ実測で、終わるまで最長 0.34 秒だった。
const EXIT_LIMIT: Duration = Duration::from_secs(10);

/// 終わったかを確かめ直す間隔。
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// 接続を閉じたサーバの stderr が閉じるのを待つ上限。
///
/// 子プロセスを kill した後に待つので、普通はすぐ閉じる。閉じないのは、サーバが起こした
/// 孫プロセスが stderr を握ったまま残ったとき。
const STDERR_CLOSE_LIMIT: Duration = Duration::from_secs(1);

/// サーバを待つ期限の組。
///
/// **値はハードコードの 1 組だけ。** 設定ファイルへの外出しは、実際に調整したくなってから
/// （`docs/dryguard-plan.md`「Phase 3」の「実際に調整したくなった項目だけ切り出す」）。
/// 組にしてあるのは、テストが短い期限でサーバを起動できるようにするため。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WaitLimits {
    /// 何も届かないまま待つ上限。
    silence: Duration,
    /// 送ったものが受け取られないまま待つ上限。
    intake: Duration,
    /// `exit` の後に終了を待つ上限。
    exit: Duration,
}

impl WaitLimits {
    const HARDCODED: Self = Self {
        silence: SILENCE_LIMIT,
        intake: INTAKE_LIMIT,
        exit: EXIT_LIMIT,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ServerLanguage {
    TypeScript,
    Rust,
}

/// どの LSP サーバをどう起動するか。
///
/// **起動方法・プロジェクトの印・言語はこの値で揃える。**
/// (`docs/dryguard-plan.md`「LSPサーバ: TS は typescript-language-server、
/// Rust は rust-analyzer」)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCommand {
    program: String,
    args: Vec<String>,
    project_markers: Vec<String>,
    language: ServerLanguage,
    wait_limits: WaitLimits,
}

impl ServerCommand {
    /// 実行ファイル名・引数・プロジェクトの印から起動の仕方を組み立てる。
    ///
    /// `project_markers` はそのサーバがプロジェクトの根と見なすファイルの名前。
    /// **印で範囲を決めないサーバには空を渡す。** 根が範囲そのものになるので、
    /// 参照元は揃う扱いになる（`WorkspaceRoot::enclosing_project`）。
    /// 独自サーバは TypeScript の問い合わせ形式を使う。Rust は [`Self::rust`] を使う。
    pub fn new(
        program: impl Into<String>,
        args: Vec<String>,
        project_markers: Vec<String>,
    ) -> Self {
        Self {
            program: program.into(),
            args,
            project_markers,
            language: ServerLanguage::TypeScript,
            wait_limits: WaitLimits::HARDCODED,
        }
    }

    /// typescript-language-server を stdio で起動する指定。
    pub fn typescript() -> Self {
        Self::new(
            TYPESCRIPT_SERVER,
            vec![STDIO_OPTION.to_owned()],
            TYPESCRIPT_PROJECT_MARKERS.map(str::to_owned).to_vec(),
        )
    }

    /// rust-analyzer を stdio で起動する指定。
    ///
    /// rust-analyzer は引数を付けずに起動すると LSP サーバになる。
    pub fn rust() -> Self {
        Self {
            language: ServerLanguage::Rust,
            ..Self::new(
                RUST_SERVER,
                Vec::new(),
                RUST_PROJECT_MARKERS.map(str::to_owned).to_vec(),
            )
        }
    }

    /// 実行ファイル名。
    pub fn program(&self) -> &str {
        &self.program
    }

    /// そのサーバがプロジェクトの根と見なすファイルの名前。
    ///
    /// 拡張子の一覧を `Grammar` が 1 箇所で持つのと同じで、**言語ごとに決まる情報**
    /// なのでサーバの指定と一緒に置く（`rules/naming.md`「このツールの語彙を固定する」）。
    pub fn project_markers(&self) -> &[String] {
        &self.project_markers
    }

    /// このサーバで問い合わせられるソースの言語。
    pub(crate) fn supports(&self, grammar: Grammar) -> bool {
        match self.language {
            ServerLanguage::TypeScript => grammar != Grammar::Rust,
            ServerLanguage::Rust => grammar == Grammar::Rust,
        }
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
    connection: Connection<BufReader<SilenceLimitedReader>, IntakeLimitedWriter>,
    stderr: StderrTail,
    program: String,
    wait_limits: WaitLimits,
    terminated: bool,
    /// 沈黙の上限を超えたか。超えたサーバは kill 済みで、以後は送らずに断る。
    ///
    /// **Why（断る）**: kill しても、サーバが起こした孫プロセスがパイプを握ったまま残ると
    /// 読み口は閉じない。送れば、もう 1 度期限まで待つ。理由も `ServerUnresponsive` のまま
    /// 保てる（`pipeline` は 1 つが落ちても残りを尋ねるので、後続は必ず来る）。
    ///
    /// **受け取りの上限はここで覚えない。** 書き口（[`IntakeLimitedWriter`]）自身が、
    /// 超えた後の書き込みを待たずに断る。
    unresponsive: bool,
}

impl Client {
    /// サーバを起動し、stdin / stdout / stderr を配線する。
    ///
    /// stdout と stderr は別スレッドが吸い続ける。stdout は期限付きで受け取り、
    /// stderr は末尾だけを持つ。stderr をこちらの出力に混ぜないのは、サーバのログで
    /// 結果が読めなくなるため。起動直後に黙った場合は、その末尾を
    /// [`ClientError::ServerClosedDuringHandshake`] に載せる。
    ///
    /// # Errors
    ///
    /// 実行ファイルが見つからないとき、起動できないとき、パイプを取り出せないとき。
    pub fn start(command: &ServerCommand) -> Result<Self, ClientError> {
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|cause| spawn_error_of(&command.program, cause))?;

        let pipes = child
            .stdin
            .take()
            .zip(child.stdout.take())
            .zip(child.stderr.take());
        let Some(((stdin, stdout), stderr)) = pipes else {
            // piped を指定した以上ここへは来ないが、来たときに子プロセスを残さない。
            let _ = child.kill();
            let _ = child.wait();
            return Err(ClientError::PipesNotWired);
        };

        let input = match IntakeLimitedWriter::spawn(stdin, command.wait_limits.intake) {
            Ok(input) => input,
            Err(cause) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ClientError::InputNotForwarded(cause));
            }
        };

        let drained = SilenceLimitedReader::spawn(stdout, command.wait_limits.silence)
            .and_then(|output| Ok((output, StderrTail::spawn(stderr)?)));
        let (output, stderr) = match drained {
            Ok(drained) => drained,
            Err(cause) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ClientError::OutputNotDrained(cause));
            }
        };

        Ok(Self {
            child,
            connection: Connection::new(BufReader::new(output), input)
                .with_language(command.language),
            stderr,
            program: command.program.clone(),
            wait_limits: command.wait_limits,
            terminated: false,
            unresponsive: false,
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
    /// 往復が失敗したとき。サーバが答えないまま接続を閉じた場合は
    /// [`ClientError::ServerClosedDuringHandshake`]、何も送ってこないまま期限を過ぎた場合は
    /// [`ClientError::ServerUnresponsive`]、送ったものを受け取らないまま期限を過ぎた場合は
    /// [`ClientError::ServerNotReading`]。抜けた [`Client`] は `Drop` が kill する。
    pub fn handshake(mut self, root: &WorkspaceRoot) -> Result<Session, ClientError> {
        let capabilities = match self.connection.handshake(root) {
            Ok(capabilities) => capabilities,
            Err(cause) => return Err(self.handshake_error_of(cause)),
        };

        Ok(Session {
            client: self,
            capabilities,
        })
    }
}

impl Client {
    /// 握手の失敗を、サーバが黙った場合とそれ以外に分ける。
    ///
    /// フレームの切れ目の EOF と、`initialize` を書き込めない（`BrokenPipe`）のは、どちらも
    /// 「起動はしたが、答えないまま接続を閉じた」。利用者が次に試すこと（stderr を見る・
    /// サーバを直接起動する）は他の失敗と違うので、専用のバリアントで返す。
    ///
    /// **Why（書き込みの失敗も含める）**: 起動直後に死ぬサーバは、こちらが `initialize` を
    /// 書く前に死ぬことがある。EOF だけを見ると、どちらが先かで stderr の末尾が消える。
    ///
    /// **Why not（`Child::try_wait` で終了を確かめてから名乗る）**: EOF の直後は、
    /// 終了したサーバでもまだ回収できていないことがある。確かめたつもりで取り違えるより、
    /// **観測した事実（接続が閉じた）だけを名前にする**
    /// (rules/naming.md「名前と実体を一致させる」)。
    fn handshake_error_of(&mut self, cause: ConnectionError) -> ClientError {
        if !is_connection_closed(&cause) {
            return self.conversation_error_of(cause);
        }

        // 先に止める。止めると stderr が閉じ、末尾を読み切ってから取り出せる。
        self.terminate();

        ClientError::ServerClosedDuringHandshake {
            program: self.program.clone(),
            stderr_tail: self.stderr.tail_text_within(STDERR_CLOSE_LIMIT),
        }
    }

    /// 往復を 1 つ行う。期限を超えたサーバには送らずに断る（受け取りの上限は書き口が断る）。
    ///
    /// **`ClientError` への直し方はここに 1 つだけ置く。** 往復ごとに直すと、
    /// 期限の見分けが漏れた経路だけ `Conversation` のまま出る。
    ///
    /// # Errors
    ///
    /// 既に黙り込んだと分かっているとき、往復が失敗したとき。
    fn converse<T>(
        &mut self,
        exchange: impl FnOnce(
            &mut Connection<BufReader<SilenceLimitedReader>, IntakeLimitedWriter>,
        ) -> Result<T, ConnectionError>,
    ) -> Result<T, ClientError> {
        if self.unresponsive {
            return Err(self.unresponsive_error());
        }

        exchange(&mut self.connection).map_err(|cause| self.conversation_error_of(cause))
    }

    /// 往復の失敗を、期限を超えた場合（どの期限か）とそれ以外に分ける。
    ///
    /// **超えたらその場で kill する。** 待ちをやめた後も生かしておくと、固まったサーバが
    /// `Drop` まで残る。
    fn conversation_error_of(&mut self, cause: ConnectionError) -> ClientError {
        if is_silence_exceeded(&cause) {
            self.unresponsive = true;
            self.terminate();
            return self.unresponsive_error();
        }

        if is_intake_exceeded(&cause) {
            self.terminate();
            return ClientError::ServerNotReading {
                program: self.program.clone(),
                waited: self.wait_limits.intake,
            };
        }

        ClientError::Conversation(cause)
    }

    fn unresponsive_error(&self) -> ClientError {
        ClientError::ServerUnresponsive {
            program: self.program.clone(),
            silence: self.wait_limits.silence,
        }
    }

    /// `exit` を送った後、子プロセスが終わるのを期限まで待つ。
    ///
    /// **Why not（`Child::wait`）**: `exit` を受け取っても終わらないサーバの前で止まり続ける。
    ///
    /// # Errors
    ///
    /// 終了を確かめられなかったとき、期限までに終わらなかったとき（kill してから返す）。
    fn exit_status_within_limit(&mut self) -> Result<ExitStatus, ClientError> {
        let started = Instant::now();

        loop {
            if let Some(status) = self.child.try_wait().map_err(ClientError::Wait)? {
                self.terminated = true;
                return Ok(status);
            }

            if started.elapsed() >= self.wait_limits.exit {
                self.terminate();
                return Err(ClientError::ExitTimedOut {
                    program: self.program.clone(),
                    waited: self.wait_limits.exit,
                });
            }

            thread::sleep(EXIT_POLL_INTERVAL);
        }
    }

    /// 子プロセスを kill して回収する。既に終わっていれば何もしない。
    ///
    /// 失敗しても報告先が無いので捨てる（`Drop` と同じ）。出力を吸うスレッドは join しない。
    /// 孫プロセスがパイプを握っていると終わらず、そこで止まる。
    fn terminate(&mut self) {
        if self.terminated {
            return;
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
        self.terminated = true;
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
    /// Rust は Cargo.toml でワークスペースを決め、tsserver の projectInfo を持たない。
    pub(crate) fn needs_project_membership_query(&self) -> bool {
        self.client.connection.language() == ServerLanguage::TypeScript
    }
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
            .converse(|connection| connection.open_document(document))
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
            .converse(|connection| connection.hover(document, position))
    }

    /// 開かせたファイルの、指定位置に書かれた型が宣言されている場所を尋ねる。
    ///
    /// `position` は `TypeReference::position` が指す型名の位置。宣言が返ったのか、
    /// 無かったのか、読めなかったのかは [`DeclarationSiteOutcome`] が分けて持つ。
    ///
    /// **typeDefinition を提供していないサーバには送らない**
    /// （[`DeclarationSiteOutcome::NotSupported`]）。hover と同じ理由で、送ると
    /// **シグナルが取れないだけの話が往復の失敗になる**。
    ///
    /// 先に [`Session::open_document`] で開かせておく。
    ///
    /// # Errors
    ///
    /// そのドキュメントを開かせていないとき、往復が失敗したとき、
    /// 応答を typeDefinition の結果として読めないとき。
    pub fn type_definition(
        &mut self,
        document: &SourceDocument,
        position: SourcePosition,
    ) -> Result<DeclarationSiteOutcome, ClientError> {
        if !provides_type_definition(&self.capabilities) {
            return Ok(DeclarationSiteOutcome::NotSupported);
        }

        self.client
            .converse(|connection| connection.type_definition(document, position))
    }

    /// 開かせたファイルの、指定位置に書かれた名前が宣言されている場所を尋ねる。
    ///
    /// `position` は `TypeReference::position` が指す型名の位置。
    ///
    /// **typeDefinition と違い、名前そのものの宣言を返す**（型エイリアスなら
    /// エイリアスの宣言）。どちらを使うかは言語ごとに `semantics` が決める
    /// （`Connection::definition`）。
    ///
    /// **definition を提供していないサーバには送らない**
    /// （[`DeclarationSiteOutcome::NotSupported`]）。[`Session::type_definition`] と同じ理由。
    ///
    /// 先に [`Session::open_document`] で開かせておく。
    ///
    /// # Errors
    ///
    /// そのドキュメントを開かせていないとき、往復が失敗したとき、
    /// 応答を definition の結果として読めないとき。
    pub fn definition(
        &mut self,
        document: &SourceDocument,
        position: SourcePosition,
    ) -> Result<DeclarationSiteOutcome, ClientError> {
        if !provides_definition(&self.capabilities) {
            return Ok(DeclarationSiteOutcome::NotSupported);
        }

        self.client
            .converse(|connection| connection.definition(document, position))
    }

    /// 開かせたファイルの、指定位置に書かれた trait の impl の場所を全件尋ねる。
    ///
    /// **implementation を提供していないサーバには送らない**
    /// （[`ImplementationOutcome::NotSupported`]）。[`Session::definition`] と同じ理由。
    ///
    /// # Errors
    ///
    /// そのドキュメントを開かせていないとき、往復が失敗したとき、
    /// 応答を implementation の結果として読めないとき。
    pub(crate) fn implementation(
        &mut self,
        document: &SourceDocument,
        position: SourcePosition,
    ) -> Result<ImplementationOutcome, ClientError> {
        if !provides_implementation(&self.capabilities) {
            return Ok(ImplementationOutcome::NotSupported);
        }
        self.client
            .connection
            .implementation(document, position)
            .map_err(ClientError::Conversation)
    }

    /// 定数の宣言を一意に選べるかを含めて尋ねる。
    ///
    /// # Errors
    ///
    /// 未開封のドキュメント、往復または応答の解釈に失敗したとき。
    pub(crate) fn unique_definition(
        &mut self,
        document: &SourceDocument,
        position: SourcePosition,
    ) -> Result<UniqueDeclarationSiteOutcome, ClientError> {
        if !provides_definition(&self.capabilities) {
            return Ok(UniqueDeclarationSiteOutcome::Unambiguous(
                DeclarationSiteOutcome::NotSupported,
            ));
        }
        self.client
            .converse(|connection| connection.unique_definition(document, position))
    }

    /// 宣言の場所を指して、そこにある名前の型の綴りを尋ねる。
    ///
    /// `site` は [`Session::type_definition`] が返した場所。型エイリアスの右辺は、
    /// **使用側ではなく宣言の位置へ尋ねないと返らない**（使用側では `import Amount`
    /// としか返らない）。
    ///
    /// **宣言のあるファイルは先に開かせておく。** 開かせていないと、サーバは綴りを
    /// 持たない応答を返す（[`HoverOutcome::Unreadable`] になる）。
    ///
    /// # Errors
    ///
    /// 往復が失敗したとき、応答を hover の結果として読めないとき。
    pub fn hover_at_declaration(
        &mut self,
        site: &DeclarationSite,
    ) -> Result<HoverOutcome, ClientError> {
        if !provides_hover(&self.capabilities) {
            return Ok(HoverOutcome::NotSupported);
        }

        self.client
            .converse(|connection| connection.hover_at_declaration(site))
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
            .converse(|connection| connection.references(document, position))
    }

    /// そのサーバが references に答えるか。
    ///
    /// **尋ねる前に分かる。** 答えに効かない問い合わせを省くのに使う
    /// （references に答えないサーバでは、所属を確かめても呼び出し元は取れない）。
    pub(crate) fn answers_references(&self) -> bool {
        provides_references(&self.capabilities)
    }

    /// 開かせたファイルの、指定位置にある名前が呼んでいる相手を尋ねる。
    ///
    /// `position` は `Chunk::name_position` が指す識別子の位置。呼び出し先が返ったのか、
    /// 起点が無かったのか、読めなかったのかは [`CalleesOutcome`] が分けて持つ。
    ///
    /// **callHierarchy を提供していないサーバには送らない**（[`CalleesOutcome::NotSupported`]）。
    /// hover と同じ理由で、送ると**シグナルが取れないだけの話が往復の失敗になる**。
    ///
    /// 先に [`Session::open_document`] で開かせておく。
    ///
    /// # Errors
    ///
    /// そのドキュメントを開かせていないとき、往復が失敗したとき、
    /// 応答を callHierarchy の結果として読めないとき。
    pub fn callees(
        &mut self,
        document: &SourceDocument,
        position: SourcePosition,
    ) -> Result<CalleesOutcome, ClientError> {
        if !provides_call_hierarchy(&self.capabilities) {
            return Ok(CalleesOutcome::NotSupported);
        }

        self.client
            .converse(|connection| connection.callees(document, position))
    }

    /// そのファイルを、サーバがどのプロジェクトの一員として扱っているかを尋ねる。
    ///
    /// hover と同じく、**サーバができると宣言していなければ送らない**。
    /// 送ってしまうと、尋ねる手立てが無いだけの話が往復の失敗になる。
    ///
    /// 先に [`Session::open_document`] で開かせておく。
    ///
    /// # Errors
    ///
    /// そのドキュメントを開かせていないとき、往復が失敗したとき、
    /// 応答からプロジェクトの綴りを読めないとき。
    pub(crate) fn project_membership(
        &mut self,
        document: &SourceDocument,
    ) -> Result<ProjectMembershipOutcome, ClientError> {
        if !provides_tsserver_requests(&self.capabilities) {
            return Ok(ProjectMembershipOutcome::NotSupported);
        }

        self.client
            .converse(|connection| connection.project_membership(document))
    }

    /// 開かせたファイルを閉じさせる。開いていなければ何もしない。
    ///
    /// # Errors
    ///
    /// 送信が失敗したとき。
    pub fn close_document(&mut self, document: &SourceDocument) -> Result<(), ClientError> {
        self.client
            .converse(|connection| connection.close_document(document))
    }

    /// サーバを終わらせ、子プロセスの終了を待つ。
    ///
    /// 値を取るのは、終わらせた後の [`Session`] を残さないため
    /// (rules/coding.md「不正な状態を型で表現できなくする」)。
    ///
    /// # Errors
    ///
    /// 往復が失敗したとき、終了を待てなかったとき、期限までに終わらなかったとき、
    /// サーバが異常終了したとき。往復の失敗で抜けた場合は `Drop` が kill する。
    pub fn shutdown(mut self) -> Result<(), ClientError> {
        self.client.converse(|connection| connection.shutdown())?;
        let status = self.client.exit_status_within_limit()?;

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
        self.terminate();
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

/// そのサーバが typeDefinition に答えるか。
///
/// hover と同じく**有無ではなく中身を見る**。無効を表す `Simple(false)` も
/// 「宣言はある」ので、`is_some()` で見ると typeDefinition を切ったサーバへ送ってしまう。
fn provides_type_definition(capabilities: &ServerCapabilities) -> bool {
    matches!(
        capabilities.type_definition_provider,
        Some(
            TypeDefinitionProviderCapability::Simple(true)
                | TypeDefinitionProviderCapability::Options(_)
        )
    )
}

/// そのサーバが definition に答えるか。
///
/// references と同じく**有無ではなく中身を見る**。無効を表す `Left(false)` も「宣言はある」。
fn provides_definition(capabilities: &ServerCapabilities) -> bool {
    matches!(
        capabilities.definition_provider,
        Some(OneOf::Left(true) | OneOf::Right(_))
    )
}

/// そのサーバが implementation に答えるか。
///
/// definition と同じく**有無ではなく中身を見る**。無効を表す `Simple(false)` も「宣言はある」。
fn provides_implementation(capabilities: &ServerCapabilities) -> bool {
    matches!(
        capabilities.implementation_provider,
        Some(ImplementationProviderCapability::Simple(true))
            | Some(ImplementationProviderCapability::Options(_))
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

/// そのサーバが callHierarchy に答えるか。
///
/// hover と同じく**有無ではなく中身を見る**。無効を表す `Simple(false)` も
/// 「宣言はある」ので、`is_some()` で見ると callHierarchy を切ったサーバへ送ってしまう。
fn provides_call_hierarchy(capabilities: &ServerCapabilities) -> bool {
    matches!(
        capabilities.call_hierarchy_provider,
        Some(
            CallHierarchyServerCapability::Simple(true) | CallHierarchyServerCapability::Options(_)
        )
    )
}

/// そのサーバが、tsserver への要求を通す口を提供するか。
///
/// **LSP の標準にプロジェクト所属を返す要求が無い**ので、hover のように専用の
/// capability を見られない。代わりに、その口を広告しているかを見る
/// （`executeCommandProvider.commands` に載る）。
///
/// **Why not（サーバの名前で見分ける）**: `ServerCommand` の実行ファイル名は
/// 利用者が差し替えられる。名前で決めると、別名で入れた同じサーバに送らなくなり、
/// **同じサーバなのに答えが変わる**。
fn provides_tsserver_requests(capabilities: &ServerCapabilities) -> bool {
    capabilities
        .execute_command_provider
        .as_ref()
        .is_some_and(|provider| {
            provider
                .commands
                .iter()
                .any(|command| command == project_membership::TSSERVER_REQUEST_COMMAND)
        })
}

/// 往復の失敗が、サーバが接続を閉じたことによるものか。
fn is_connection_closed(cause: &ConnectionError) -> bool {
    matches!(cause, ConnectionError::Framing(FramingError::ServerClosed))
        || matches!(cause, ConnectionError::Send(sent) if sent.kind() == io::ErrorKind::BrokenPipe)
}

/// 往復の失敗が、沈黙の上限を超えたことによるものか。
///
/// **`framing` に専用のバリアントを置かない。** 期限は区切りの話ではなく読み口
/// （[`SilenceLimitedReader`]）の性質で、読み取りで `TimedOut` を返すのはこの読み口だけ。
fn is_silence_exceeded(cause: &ConnectionError) -> bool {
    matches!(
        cause,
        ConnectionError::Framing(FramingError::Read(read)) if read.kind() == io::ErrorKind::TimedOut
    )
}

/// 往復の失敗が、受け取りの上限を超えたことによるものか。
///
/// 書き込みで `TimedOut` を返すのは書き口（[`IntakeLimitedWriter`]）だけ。
fn is_intake_exceeded(cause: &ConnectionError) -> bool {
    matches!(cause, ConnectionError::Send(sent) if sent.kind() == io::ErrorKind::TimedOut)
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
    /// 子プロセスの出力を吸うスレッドを作れなかった。
    OutputNotDrained(io::Error),
    /// 子プロセスの入力へ書き込むスレッドを作れなかった。
    InputNotForwarded(io::Error),
    /// 起動はしたが、握手に答えないまま接続（stdout か stdin）を閉じた。
    ///
    /// 子プロセスが終了したかまでは見ていない。閉じた時点で会話は続けられないので、
    /// どちらでも kill する。
    ServerClosedDuringHandshake {
        /// 黙った実行ファイル名。
        program: String,
        /// サーバが stderr に書いた末尾。何も書かなければ空。
        stderr_tail: String,
    },
    /// 何も送ってこないまま、沈黙の上限を超えた。サーバは kill 済み。
    ServerUnresponsive {
        /// 黙り込んだ実行ファイル名。
        program: String,
        /// 待った長さ。
        silence: Duration,
    },
    /// 送ったものを受け取らないまま、上限を超えた。サーバは kill 済み。
    ///
    /// **`ServerUnresponsive` と分ける。** stdin を読まないままログを流し続けるサーバもあり、
    /// 「何も送ってこない」とは限らない。
    ServerNotReading {
        /// 受け取らなかった実行ファイル名。
        program: String,
        /// 1 塊が受け取られるのを待った長さ。
        waited: Duration,
    },
    /// `exit` を送った後、期限までに終わらなかった。サーバは kill 済み。
    ExitTimedOut {
        /// 終わらなかった実行ファイル名。
        program: String,
        /// 待った長さ。
        waited: Duration,
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
            // stderr に何も残っていなければ、次に試すことだけを出す。
            Self::ServerClosedDuringHandshake {
                program,
                stderr_tail,
            } => {
                write!(
                    formatter,
                    "LSP サーバ ({program}) が握手に答えないまま接続を閉じました。\
                     {program} を直接起動して、起動時のエラーを確認してください"
                )?;
                if stderr_tail.is_empty() {
                    return Ok(());
                }
                write!(formatter, "\n{program} の stderr の末尾:\n{stderr_tail}")
            }
            Self::ServerUnresponsive { program, silence } => write!(
                formatter,
                "LSP サーバ ({program}) が {} 秒のあいだ何も送ってこないため、止めました",
                silence.as_secs_f64()
            ),
            Self::ServerNotReading { program, waited } => write!(
                formatter,
                "LSP サーバ ({program}) が {} 秒のあいだ送ったものを受け取らないため、止めました",
                waited.as_secs_f64()
            ),
            Self::ExitTimedOut { program, waited } => write!(
                formatter,
                "LSP サーバ ({program}) が終了の通知から {} 秒たっても終わらないため、止めました",
                waited.as_secs_f64()
            ),
            Self::OutputNotDrained(cause) => write!(
                formatter,
                "LSP サーバの出力を読むスレッドを作れません: {cause}"
            ),
            Self::InputNotForwarded(cause) => write!(
                formatter,
                "LSP サーバへ書き込むスレッドを作れません: {cause}"
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
            | Self::ServerUnresponsive { .. }
            | Self::ServerNotReading { .. }
            | Self::ExitTimedOut { .. }
            | Self::AbnormalExit { .. } => None,
            Self::Spawn { cause, .. } => Some(cause),
            Self::Conversation(cause) => Some(cause),
            Self::Wait(cause) | Self::OutputNotDrained(cause) | Self::InputNotForwarded(cause) => {
                Some(cause)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::time::Instant;

    use crate::codebase;
    use crate::test_support::{line, repository_path, signature_text};
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

    /// 2 つのモジュールを呼んでいる関数を持つ fixture。
    ///
    /// **[`AN_UNCALLED_FILE`] と同じファイル。** 呼び出し元が 1 件も無い関数が
    /// 呼び出し先を 2 件持つので、**向きが逆であること**が 1 つの入力から出る。
    const A_CALLING_FILE: &str = AN_UNCALLED_FILE;

    /// 何も呼んでいない関数を持つ fixture。
    const A_FILE_CALLING_NOTHING: &str = "tests/fixtures/references/src/report/paramValue.ts";

    /// 2 つの fixture から決まる、サーバに見せる根。
    ///
    /// **テスト側で広げない。** `WorkspaceRoot::enclosing` は綴りだけで根を決める
    /// （実在を確かめない）ので、置いていないパスを混ぜると黙って根が広がり、
    /// **本番が作らない設定でテストが通る**。2 つの共通の祖先
    /// （`tests/fixtures/references/src`）に tsconfig.json を置いてあり、そこが根になる。
    fn references_fixture_root() -> WorkspaceRoot {
        WorkspaceRoot::enclosing(&[
            repository_path(A_CALLED_FILE),
            repository_path(AN_UNCALLED_FILE),
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

    /// 開かせたファイルの、その位置にある名前が呼んでいる相手をサーバに尋ねる。
    fn callees_at(relative_path: &str, position: SourcePosition) -> CalleesOutcome {
        let client = Client::start(&ServerCommand::typescript()).expect("サーバを起動できる");
        let mut session = client
            .handshake(&references_fixture_root())
            .expect("握手できる");
        let document = document_of(relative_path);
        session.open_document(&document).expect("開かせられる");

        let outcome = session
            .callees(&document, position)
            .expect("問い合わせられる");

        session.shutdown().expect("終了できる");
        outcome
    }

    #[test]
    fn test_client_start_with_a_missing_program_reports_the_server_not_found() {
        let command =
            ServerCommand::new("dryguard-no-such-language-server", Vec::new(), Vec::new());

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

    /// そのサーバができることとして typeDefinition だけを宣言した capabilities。
    fn capabilities_declaring_type_definition(
        type_definition_provider: Option<TypeDefinitionProviderCapability>,
    ) -> ServerCapabilities {
        ServerCapabilities {
            type_definition_provider,
            ..ServerCapabilities::default()
        }
    }

    #[test]
    fn test_provides_type_definition_with_a_server_that_declares_it_is_true() {
        let capabilities = capabilities_declaring_type_definition(Some(
            TypeDefinitionProviderCapability::Simple(true),
        ));

        assert!(provides_type_definition(&capabilities));
    }

    #[test]
    fn test_provides_type_definition_with_a_server_that_turned_it_off_is_false() {
        // 対照は上のテスト。**宣言はあるが無効**という形で、`is_some()` で見ていると
        // typeDefinition を切ったサーバへ要求を送ってしまう
        let capabilities = capabilities_declaring_type_definition(Some(
            TypeDefinitionProviderCapability::Simple(false),
        ));

        assert!(!provides_type_definition(&capabilities));
    }

    #[test]
    fn test_provides_type_definition_with_a_server_that_does_not_declare_it_is_false() {
        let capabilities = capabilities_declaring_type_definition(None);

        assert!(!provides_type_definition(&capabilities));
    }

    /// そのサーバができることとして definition だけを宣言した capabilities。
    fn capabilities_declaring_definition(
        definition_provider: Option<OneOf<bool, lsp_types::DefinitionOptions>>,
    ) -> ServerCapabilities {
        ServerCapabilities {
            definition_provider,
            ..ServerCapabilities::default()
        }
    }

    #[test]
    fn test_provides_definition_with_a_server_that_declares_it_is_true() {
        let capabilities = capabilities_declaring_definition(Some(OneOf::Left(true)));

        assert!(provides_definition(&capabilities));
    }

    #[test]
    fn test_provides_definition_with_a_server_that_turned_it_off_is_false() {
        // 対照は上のテスト。**宣言はあるが無効**という形で、`is_some()` で見ていると
        // definition を切ったサーバへ要求を送ってしまう
        let capabilities = capabilities_declaring_definition(Some(OneOf::Left(false)));

        assert!(!provides_definition(&capabilities));
    }

    #[test]
    fn test_provides_definition_with_a_server_that_does_not_declare_it_is_false() {
        let capabilities = capabilities_declaring_definition(None);

        assert!(!provides_definition(&capabilities));
    }

    #[test]
    fn test_provides_implementation_with_a_server_that_turned_it_off_is_false() {
        let declared = |implementation_provider| ServerCapabilities {
            implementation_provider,
            ..ServerCapabilities::default()
        };

        assert!(provides_implementation(&declared(Some(
            ImplementationProviderCapability::Simple(true)
        ))));
        assert!(!provides_implementation(&declared(Some(
            ImplementationProviderCapability::Simple(false)
        ))));
        assert!(!provides_implementation(&declared(None)));
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

    /// そのサーバができることとして callHierarchy だけを宣言した capabilities。
    fn capabilities_declaring_call_hierarchy(
        call_hierarchy_provider: Option<CallHierarchyServerCapability>,
    ) -> ServerCapabilities {
        ServerCapabilities {
            call_hierarchy_provider,
            ..ServerCapabilities::default()
        }
    }

    #[test]
    fn test_provides_call_hierarchy_with_a_server_that_declares_it_is_true() {
        let capabilities = capabilities_declaring_call_hierarchy(Some(
            CallHierarchyServerCapability::Simple(true),
        ));

        assert!(provides_call_hierarchy(&capabilities));
    }

    #[test]
    fn test_provides_call_hierarchy_with_a_server_that_turned_it_off_is_false() {
        // 対照は上のテスト。**宣言はあるが無効**という形で、`is_some()` で見ていると
        // callHierarchy を切ったサーバへ要求を送ってしまう
        let capabilities = capabilities_declaring_call_hierarchy(Some(
            CallHierarchyServerCapability::Simple(false),
        ));

        assert!(!provides_call_hierarchy(&capabilities));
    }

    #[test]
    fn test_provides_call_hierarchy_with_a_server_that_does_not_declare_it_is_false() {
        let capabilities = capabilities_declaring_call_hierarchy(None);

        assert!(!provides_call_hierarchy(&capabilities));
    }

    /// そのサーバが実行できるコマンドとして、渡した綴りだけを宣言した capabilities。
    fn capabilities_declaring_commands(commands: &[&str]) -> ServerCapabilities {
        ServerCapabilities {
            execute_command_provider: Some(lsp_types::ExecuteCommandOptions {
                commands: commands
                    .iter()
                    .map(|command| (*command).to_owned())
                    .collect(),
                work_done_progress_options: lsp_types::WorkDoneProgressOptions::default(),
            }),
            ..ServerCapabilities::default()
        }
    }

    #[test]
    fn test_provides_tsserver_requests_with_a_server_that_declares_the_command_is_true() {
        let capabilities =
            capabilities_declaring_commands(&[project_membership::TSSERVER_REQUEST_COMMAND]);

        assert!(provides_tsserver_requests(&capabilities));
    }

    #[test]
    fn test_provides_tsserver_requests_with_a_server_declaring_only_other_commands_is_false() {
        // 対照は上のテスト。**一覧の有無ではなく中身を見る**。`is_some()` で見ていると、
        // 別のコマンドだけを持つサーバへ tsserver 宛の要求を送ってしまう
        let capabilities = capabilities_declaring_commands(&["_typescript.organizeImports"]);

        assert!(!provides_tsserver_requests(&capabilities));
    }

    #[test]
    fn test_provides_tsserver_requests_with_a_server_that_declares_no_commands_is_false() {
        let capabilities = ServerCapabilities::default();

        assert!(!provides_tsserver_requests(&capabilities));
    }

    /// 期限に触れることを確かめるテストだけが使う、短い上限。
    ///
    /// **黙る相手は、何も送らない・終わらないことが台本で決まっている**ので、短くても
    /// 起動の遅れで答えが変わらない。
    const SHORT_LIMIT: Duration = Duration::from_millis(300);

    /// 握手に答えてから黙るサーバの沈黙の上限。握手の応答が、起動の遅れを含めて
    /// この間に届けばよい。
    const AFTER_HANDSHAKE_LIMIT: Duration = Duration::from_secs(2);

    /// 届くはずのものが届かないときだけ効く、テストの上限。
    const GENEROUS_LIMIT: Duration = Duration::from_secs(10);

    /// 期限に触れないはずのテストが使う期限の組。
    const GENEROUS_LIMITS: WaitLimits = WaitLimits {
        silence: GENEROUS_LIMIT,
        intake: GENEROUS_LIMIT,
        exit: GENEROUS_LIMIT,
    };

    /// `sh -c` で台本どおりに振る舞う、LSP サーバのフェイク。
    ///
    /// **孫プロセスを作らないよう、黙るときは `exec sleep` にする。** 孫がパイプを握ると、
    /// テスト自身が「子プロセスを残さない」を破る。
    fn fake_server(script: &str, wait_limits: WaitLimits) -> ServerCommand {
        ServerCommand {
            wait_limits,
            ..ServerCommand::new("sh", vec!["-c".to_owned(), script.to_owned()], Vec::new())
        }
    }

    /// payload を 1 フレームとして stdout へ書く台本。
    fn printed_frame(payload: &str) -> String {
        format!(
            "printf 'Content-Length: {}\\r\\n\\r\\n%s' '{payload}'",
            payload.len()
        )
    }

    /// 1 通目の要求（`initialize`）への応答。hover を提供すると答える。
    const INITIALIZE_RESPONSE: &str =
        r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"hoverProvider":true}}}"#;

    /// 2 通目の要求（握手の直後の `shutdown`）への応答。
    const SHUTDOWN_RESPONSE: &str = r#"{"jsonrpc":"2.0","id":2,"result":null}"#;

    /// 台本のサーバを起動して握手させた結果。
    fn handshake_with(script: &str, wait_limits: WaitLimits) -> Result<Session, ClientError> {
        Client::start(&fake_server(script, wait_limits))
            .expect("起動できる")
            .handshake(&fixture_workspace_root())
    }

    #[test]
    #[cfg(unix)]
    fn test_client_handshake_with_a_server_that_stays_silent_reports_it_unresponsive() {
        // 起動したまま何も送ってこないサーバ。期限が無ければ、ここで止まり続ける
        let limits = WaitLimits {
            silence: SHORT_LIMIT,
            ..GENEROUS_LIMITS
        };

        let started = Instant::now();
        let error = handshake_with("exec sleep 30", limits).expect_err("握手に答えない");

        assert!(
            matches!(
                &error,
                ClientError::ServerUnresponsive { program, silence }
                    if program == "sh" && *silence == SHORT_LIMIT
            ),
            "{error:?}"
        );
        assert!(started.elapsed() < GENEROUS_LIMIT, "期限で戻る");
    }

    #[test]
    #[cfg(unix)]
    fn test_session_after_the_server_went_silent_refuses_without_waiting_again() {
        // 握手には答えたが、その後に黙り込んだサーバ。`pipeline` は 1 つが落ちても
        // 残りを尋ねるので、断らないと問い合わせの数だけ期限を待つ
        let script = format!("{}; exec sleep 30", printed_frame(INITIALIZE_RESPONSE));
        let limits = WaitLimits {
            silence: AFTER_HANDSHAKE_LIMIT,
            ..GENEROUS_LIMITS
        };
        let mut session = handshake_with(&script, limits).expect("握手できる");
        let document = fixture_document();
        session.open_document(&document).expect("開かせられる");
        let position = SourcePosition::from_preceding_text(line(5), "export function ");

        let first = session.hover(&document, position);
        let started = Instant::now();
        let second = session.hover(&document, position);
        let refused_in = started.elapsed();

        assert!(
            matches!(first, Err(ClientError::ServerUnresponsive { .. })),
            "{first:?}"
        );
        assert!(
            matches!(second, Err(ClientError::ServerUnresponsive { .. })),
            "{second:?}"
        );
        assert!(refused_in < AFTER_HANDSHAKE_LIMIT, "2 度目は待たずに断る");
        let shutdown = session.shutdown();
        assert!(
            matches!(shutdown, Err(ClientError::ServerUnresponsive { .. })),
            "{shutdown:?}"
        );
    }

    /// パイプのバッファを超える中身のドキュメント。読まない相手には書き切れない。
    fn larger_than_pipe_buffer_document() -> SourceDocument {
        SourceDocument::new(
            &repository_path(A_CANDIDATE_PAIR_FILE),
            "x".repeat(child_input::MORE_THAN_PIPE_BUFFER),
        )
        .expect("ドキュメントにできる")
    }

    #[test]
    #[cfg(unix)]
    fn test_session_opening_a_large_document_on_a_server_that_does_not_read_reports_it() {
        // 握手には答えたが、その後 stdin を読まないサーバ。パイプのバッファを超えるドキュメントは
        // 書き切れず、期限が無ければ `didOpen` の書き込みで止まり続ける
        let script = format!("{}; exec sleep 30", printed_frame(INITIALIZE_RESPONSE));
        let limits = WaitLimits {
            intake: SHORT_LIMIT,
            ..GENEROUS_LIMITS
        };
        let mut session = handshake_with(&script, limits).expect("握手できる");
        let large = larger_than_pipe_buffer_document();

        let started = Instant::now();
        let error = session.open_document(&large).expect_err("書き切れない");

        assert!(
            matches!(
                &error,
                ClientError::ServerNotReading { program, waited }
                    if program == "sh" && *waited == SHORT_LIMIT
            ),
            "{error:?}"
        );
        assert!(started.elapsed() < GENEROUS_LIMIT, "期限で戻る");
    }

    #[test]
    #[cfg(unix)]
    fn test_session_after_the_server_stopped_reading_refuses_with_the_same_reason() {
        // 見限った理由を後続にも出す。沈黙の理由に取り違えると「何も送ってこない」と嘘をつき、
        // 送り直すと、もう 1 度期限まで待つ
        let script = format!("{}; exec sleep 30", printed_frame(INITIALIZE_RESPONSE));
        let limits = WaitLimits {
            intake: SHORT_LIMIT,
            ..GENEROUS_LIMITS
        };
        let mut session = handshake_with(&script, limits).expect("握手できる");
        let large = larger_than_pipe_buffer_document();
        let _ = session.open_document(&large);

        let started = Instant::now();
        let second = session.open_document(&fixture_document());

        assert!(
            matches!(second, Err(ClientError::ServerNotReading { .. })),
            "{second:?}"
        );
        assert!(started.elapsed() < SHORT_LIMIT, "2 度目は待たずに断る");
        let shutdown = session.shutdown();
        assert!(
            matches!(shutdown, Err(ClientError::ServerNotReading { .. })),
            "{shutdown:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_session_shutdown_with_a_server_that_does_not_exit_reports_it() {
        // `shutdown` には答えるが、`exit` を受け取っても終わらないサーバ
        let script = format!(
            "{}; {}; exec sleep 30",
            printed_frame(INITIALIZE_RESPONSE),
            printed_frame(SHUTDOWN_RESPONSE)
        );
        let limits = WaitLimits {
            exit: SHORT_LIMIT,
            ..GENEROUS_LIMITS
        };
        let session = handshake_with(&script, limits).expect("握手できる");

        let started = Instant::now();
        let error = session.shutdown().expect_err("終わらない");

        assert!(
            matches!(
                &error,
                ClientError::ExitTimedOut { program, waited }
                    if program == "sh" && *waited == SHORT_LIMIT
            ),
            "{error:?}"
        );
        assert!(started.elapsed() < GENEROUS_LIMIT, "期限で戻る");
    }

    #[test]
    #[cfg(unix)]
    fn test_session_shutdown_with_a_server_that_exits_succeeds() {
        // 対照は上のテスト。期限より先に終わるサーバは通る。読まずに 1 秒で終わるが、
        // 送る 4 通はパイプのバッファに収まり、その間に書き終わる
        let script = format!(
            "{}; {}; exec sleep 1",
            printed_frame(INITIALIZE_RESPONSE),
            printed_frame(SHUTDOWN_RESPONSE)
        );
        let session = handshake_with(&script, GENEROUS_LIMITS).expect("握手できる");

        session.shutdown().expect("終了できる");
    }

    #[test]
    #[cfg(unix)]
    fn test_client_handshake_with_a_server_that_dies_writing_to_stderr_carries_its_tail() {
        // 起動時のエラーを stderr にだけ書いて死ぬサーバ
        let error = handshake_with("echo 'tsserver not found' >&2; exit 1", GENEROUS_LIMITS)
            .expect_err("握手に答えない");

        assert!(
            matches!(
                &error,
                ClientError::ServerClosedDuringHandshake { program, stderr_tail }
                    if program == "sh" && stderr_tail == "tsserver not found"
            ),
            "{error:?}"
        );
        assert!(error.to_string().contains("tsserver not found"));
    }

    #[test]
    #[cfg(unix)]
    fn test_client_handshake_with_a_server_that_closed_its_input_carries_its_stderr_tail() {
        // stdin と stdout を閉じてから stderr に書くサーバ。起動直後に死ぬサーバは、こちらが
        // `initialize` を書く前に閉じることがある。閉じ終えるのを待ってから握手を始め、
        // 書き込みが `BrokenPipe` で落ちる経路を通す（間に合わなくても EOF で同じ答えになる）
        let client = Client::start(&fake_server(
            "exec 0<&- 1>&-; echo 'tsserver not found' >&2; exec sleep 30",
            GENEROUS_LIMITS,
        ))
        .expect("起動できる");
        thread::sleep(SHORT_LIMIT);

        let error = client
            .handshake(&fixture_workspace_root())
            .expect_err("握手に答えない");

        assert!(
            matches!(
                &error,
                ClientError::ServerClosedDuringHandshake { stderr_tail, .. }
                    if stderr_tail == "tsserver not found"
            ),
            "{error:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_client_handshake_with_a_server_that_dies_silently_has_no_stderr_tail() {
        // 対照は上のテスト。何も書かずに死んだサーバに、末尾をこしらえない
        let error = handshake_with("exit 1", GENEROUS_LIMITS).expect_err("握手に答えない");

        assert!(
            matches!(
                &error,
                ClientError::ServerClosedDuringHandshake { stderr_tail, .. }
                    if stderr_tail.is_empty()
            ),
            "{error:?}"
        );
        assert!(!error.to_string().contains("stderr"));
    }

    #[test]
    #[cfg(unix)]
    fn test_client_handshake_with_a_server_flooding_stderr_still_completes() {
        // パイプのバッファ（64 KiB）を超えて stderr に書いてから答えるサーバ。
        // 吸い続けていなければ、サーバが write で止まり、握手は沈黙の上限で落ちる
        let script = format!(
            "head -c 262144 /dev/zero | tr '\\0' x >&2; {}; exec sleep 30",
            printed_frame(INITIALIZE_RESPONSE)
        );

        let session = handshake_with(&script, GENEROUS_LIMITS);

        assert!(session.is_ok(), "握手できる: {:?}", session.err());
    }

    #[test]
    #[cfg(unix)]
    fn test_client_handshake_with_a_broken_frame_stays_a_conversation_error() {
        // 「サーバが黙った」以外まで起動失敗や沈黙として畳むと、直す先を取り違える
        let error = handshake_with(
            "printf 'Bogus: 1\\r\\n\\r\\n'; exec sleep 30",
            GENEROUS_LIMITS,
        )
        .expect_err("フレームを読めない");

        assert!(
            matches!(
                &error,
                ClientError::Conversation(ConnectionError::Framing(
                    FramingError::MissingContentLength
                ))
            ),
            "{error:?}"
        );
    }

    #[test]
    fn test_server_command_for_typescript_speaks_over_stdio() {
        // --stdio が無いとサーバは使い方を表示して終わり、応答は 1 つも返らない
        let command = ServerCommand::typescript();

        assert_eq!(command.program(), "typescript-language-server");
        assert_eq!(command.args, vec!["--stdio".to_owned()]);
    }

    #[test]
    fn test_server_command_for_typescript_marks_projects_with_tsconfig() {
        // 印が無いと根が候補ペアの共通の祖先のままになり、参照元が揃わない
        let command = ServerCommand::typescript();

        assert_eq!(
            command.project_markers(),
            ["tsconfig.json".to_owned(), "jsconfig.json".to_owned()]
        );
    }

    #[test]
    fn test_server_command_for_rust_uses_cargo_workspace() {
        let command = ServerCommand::rust();

        assert_eq!(command.program(), "rust-analyzer");
        assert!(command.args.is_empty(), "引数無しで stdio を使う");
        assert_eq!(command.project_markers(), ["Cargo.toml".to_owned()]);
        assert!(command.supports(Grammar::Rust));
        assert!(!command.supports(Grammar::TypeScript));
    }

    #[test]
    fn test_server_command_without_project_markers_has_none() {
        // 対照は上のテスト。**印を持たないサーバもある**ので、
        // 印の一覧が空であることと TS の一覧を取り違えない
        let command =
            ServerCommand::new("dryguard-no-such-language-server", Vec::new(), Vec::new());

        assert!(command.project_markers().is_empty());
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
            HoverOutcome::Answered(signature_text(
                "function applyDiscount(invoice: Invoice): number"
            ))
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
            .filter_map(|path| path.path().file_name()?.to_str().map(str::to_owned))
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

    #[test]
    #[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
    fn test_session_callees_at_a_function_name_answers_the_files_it_calls() {
        // fixture の 4 行目 `export function monthlyLabel`。呼んでいるのは
        // `../utils/formatDate` と `./dateHelper` の 2 つで、**別のディレクトリへ跨る**
        let outcome = callees_at(
            A_CALLING_FILE,
            SourcePosition::from_preceding_text(line(4), "export function "),
        );

        let CalleesOutcome::Answered(paths) = outcome else {
            panic!("呼び出し先のあるチャンクには呼び出し先が返る: {outcome:?}");
        };
        let names: BTreeSet<String> = paths
            .iter()
            .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
            .collect();
        assert_eq!(
            names,
            BTreeSet::from(["dateHelper.ts".to_owned(), "formatDate.ts".to_owned()])
        );
    }

    #[test]
    #[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
    fn test_session_callees_at_a_function_calling_nothing_has_no_callees() {
        // 対照は上のテスト。同じ木の中で、何も呼んでいない関数を指す。
        // 起点そのものを呼び出し先に数えていれば、ここでも 1 件返ってしまう
        let outcome = callees_at(
            A_FILE_CALLING_NOTHING,
            SourcePosition::from_preceding_text(line(1), "export function "),
        );

        assert!(
            matches!(outcome, CalleesOutcome::NoCallees),
            "何も呼んでいないチャンクでは呼び出し先が返らない: {outcome:?}"
        );
    }
}
