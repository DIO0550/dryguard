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
//! | ここ | サーバの起動・パイプの配線・終了 |
//!
//! **外へ出すのは [`ServerCommand`] / [`Client`] / [`Session`] と、失敗を読むための型だけ。**
//! 区切りや payload の組み立て方は、いつ変えても外に影響しない位置に置く
//! (rules/architecture.md「モジュールの公開 API」)。

pub(crate) mod connection;
pub(crate) mod framing;
pub(crate) mod message;

use std::error::Error;
use std::fmt;
use std::io::{self, BufReader};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};

use lsp_types::ServerCapabilities;

use connection::Connection;

// 失敗を読むための型だけを外へ出す。[`ClientError`] が抱えている以上、
// 外から名前を呼べないと `source()` をたどっても中身を見分けられない。
pub use connection::ConnectionError;
pub use framing::FramingError;
pub use message::{MessageError, RequestId, ResponseFailure};

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
    terminated: bool,
}

impl Client {
    /// サーバを起動し、stdin / stdout を配線する。
    ///
    /// stderr は捨てる。サーバのログをこちらの出力に混ぜないため。起動そのものの失敗は
    /// フレームの切れ目の EOF（[`FramingError::ServerClosed`]）として表に出る。
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
            terminated: false,
        })
    }

    /// サーバと握手し、問い合わせを送れる状態にする。
    ///
    /// 値を取るのは、握手を 2 回できないようにするため。`initialize` を 2 度送られた
    /// サーバは 2 通目を拒む。
    ///
    /// # Errors
    ///
    /// 往復が失敗したとき。抜けた [`Client`] は `Drop` が kill する。
    pub fn handshake(mut self) -> Result<Session, ClientError> {
        let capabilities = self
            .connection
            .handshake()
            .map_err(ClientError::Conversation)?;

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
            Self::ServerNotFound { .. } | Self::PipesNotWired | Self::AbnormalExit { .. } => None,
            Self::Spawn { cause, .. } => Some(cause),
            Self::Conversation(cause) => Some(cause),
            Self::Wait(cause) => Some(cause),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::HoverProviderCapability;

    #[test]
    fn test_client_start_with_a_missing_program_reports_the_server_not_found() {
        let command = ServerCommand::new("dryguard-no-such-language-server", Vec::new());

        let error = Client::start(&command).expect_err("起動できない");

        assert!(matches!(
            error,
            ClientError::ServerNotFound { program } if program == "dryguard-no-such-language-server"
        ));
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

        let session = client.handshake().expect("握手できる");

        // hover は Stage 2 で最初に使う問い合わせ。返らないサーバでは意味情報が採れない。
        // 有無ではなく中身を見る。無効を表す `Simple(false)` も「ある」なので、
        // is_some() では hover を切ったサーバでも通ってしまう
        let provides_hover = matches!(
            session.capabilities().hover_provider,
            Some(HoverProviderCapability::Simple(true) | HoverProviderCapability::Options(_))
        );

        assert!(
            provides_hover,
            "typescript-language-server は hover を提供する"
        );

        session.shutdown().expect("終了できる");
    }
}
