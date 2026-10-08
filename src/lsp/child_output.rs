//! 子プロセスの出力を別スレッドで吸い続ける。待つ側は期限付きで受け取る。
//!
//! **スレッドは子プロセスの境界にだけ置く。** [`super::connection::Connection`] は
//! `BufRead` を受けるだけで、読み口が期限を持つかを知らない。バイト列で組んだ往復の
//! テストは、スレッドもサーバも無しにそのまま書ける
//! (rules/tdd.md「`lsp` は『応答を受け取ってから先』を切り出す」)。

use std::collections::VecDeque;
use std::io::{self, Read};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

/// 吸う側のスレッドが 1 度に読む量。
const READ_CHUNK_BYTES: usize = 8 * 1024;

/// 読んだまま受け取られていない塊を溜める上限（塊の数）。
///
/// **Why not（上限なしのチャネル）**: パイプの背圧が消え、問い合わせの合間にサーバが
/// 送り続けたログの分だけ、こちらのメモリが際限なく膨らむ。上限で吸う側が止まれば、
/// パイプが埋まってサーバの側が待つ（スレッドを挟む前と同じ振る舞い）。
const PENDING_CHUNKS: usize = 64;

/// stderr から残す末尾のバイト数。
///
/// 起動直後に死んだサーバの診断は最後の数行に出る。全部を持つと、進捗を stderr へ
/// 流し続けるサーバで際限なく膨らむ。
const STDERR_TAIL_BYTES: usize = 4 * 1024;

/// 読んだ内容を、別スレッドから期限付きで受け取る読み口。
///
/// **期限は「受け取った分を読み終えて待ち始めてから」で数える。** 要求ごとの合計ではない。 rust-analyzer は読み込みの間ずっと
/// `$/progress` を流すので、沈黙で切れば正常に遅いサーバを殺さずに、固まったサーバだけを捉える。
///
/// **Why not（要求ごとの合計時間で切る）**: 落ち着くまでの合計はワークスペースの大きさで
/// 伸びる（dryguard 自身で 6.6〜24.4 秒）。合計で切ると、正常に遅いサーバを殺す側に倒れる。
#[derive(Debug)]
pub(super) struct SilenceLimitedReader {
    chunks: Receiver<io::Result<Vec<u8>>>,
    pending: Vec<u8>,
    consumed: usize,
    silence_limit: Duration,
}

impl SilenceLimitedReader {
    /// `source` を別スレッドで読み続け、`silence_limit` を超えて何も届かなければ
    /// 読み取りを [`io::ErrorKind::TimedOut`] で失敗させる読み口を作る。
    ///
    /// スレッドは `source` が尽きるか失敗すると終わる。子プロセスを kill すれば尽きる。
    ///
    /// # Errors
    ///
    /// 読むスレッドを作れないとき。
    pub(super) fn spawn<R: Read + Send + 'static>(
        source: R,
        silence_limit: Duration,
    ) -> io::Result<Self> {
        let (sender, chunks) = mpsc::sync_channel(PENDING_CHUNKS);

        thread::Builder::new().spawn(move || {
            let mut source = source;
            let mut buffer = [0; READ_CHUNK_BYTES];

            loop {
                let read = match source.read(&mut buffer) {
                    Ok(0) => return,
                    Ok(read) => read,
                    Err(cause) if cause.kind() == io::ErrorKind::Interrupted => continue,
                    Err(cause) => {
                        let _ = sender.send(Err(cause));
                        return;
                    }
                };

                // 受け手が居なくなったら、読み続けても渡す先が無い。
                if sender.send(Ok(buffer[..read].to_vec())).is_err() {
                    return;
                }
            }
        })?;

        Ok(Self {
            chunks,
            pending: Vec::new(),
            consumed: 0,
            silence_limit,
        })
    }
}

impl Read for SilenceLimitedReader {
    /// # Errors
    ///
    /// 期限を超えて何も届かなかったとき（[`io::ErrorKind::TimedOut`]）、読み取り自体が
    /// 失敗したとき。尽きたときは `Ok(0)` を返す。
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        // 0 バイトを求められたら待たない。待つと、届かないだけで失敗にしてしまう。
        if buffer.is_empty() {
            return Ok(0);
        }

        if self.consumed == self.pending.len() {
            match self.chunks.recv_timeout(self.silence_limit) {
                Ok(Ok(chunk)) => {
                    self.pending = chunk;
                    self.consumed = 0;
                }
                Ok(Err(cause)) => return Err(cause),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "{} 秒のあいだ何も届きませんでした",
                            self.silence_limit.as_secs_f64()
                        ),
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(0),
            }
        }

        let available = &self.pending[self.consumed..];
        let copied = available.len().min(buffer.len());
        buffer[..copied].copy_from_slice(&available[..copied]);
        self.consumed += copied;

        Ok(copied)
    }
}

/// サーバの stderr を吸い続け、末尾だけを持つ。
///
/// **吸い続けること自体が要る。** パイプにしたまま読まないと、バッファが埋まった時点で
/// サーバが write で止まる。
///
/// **Why not（`Stdio::null()` で捨てる）**: stderr にだけ書いて死ぬサーバの診断が消え、
/// 「出力を閉じた」しか残らない。
#[derive(Debug)]
pub(super) struct StderrTail {
    tail: Arc<Mutex<VecDeque<u8>>>,
    /// 何も送られない。吸う側のスレッドが終わると切れ、それで stderr が閉じたと分かる。
    closed: Receiver<()>,
}

impl StderrTail {
    /// `source` を別スレッドで読み続ける。
    ///
    /// # Errors
    ///
    /// 読むスレッドを作れないとき。
    pub(super) fn spawn<R: Read + Send + 'static>(source: R) -> io::Result<Self> {
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let (closing, closed) = mpsc::channel::<()>();
        let written = Arc::clone(&tail);

        thread::Builder::new().spawn(move || {
            // 終わるときに落として、待っている側へ閉じたことを伝える。
            let _closing = closing;
            let mut source = source;
            let mut buffer = [0; READ_CHUNK_BYTES];

            loop {
                let read = match source.read(&mut buffer) {
                    Ok(0) => return,
                    Ok(read) => read,
                    Err(cause) if cause.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => return,
                };

                let mut tail = written.lock().unwrap_or_else(PoisonError::into_inner);
                tail.extend(&buffer[..read]);
                let overflow = tail.len().saturating_sub(STDERR_TAIL_BYTES);
                tail.drain(..overflow);
            }
        })?;

        Ok(Self { tail, closed })
    }

    /// stderr が閉じるのを `limit` まで待ち、それまでに届いた末尾を返す。
    ///
    /// **閉じるまで待つのは、出力の閉じた直後には末尾がまだ届いていないことがあるため。**
    /// 待ち切れなければ、それまでの分を返す（サーバが起こした孫プロセスが stderr を
    /// 握ったまま残ると、閉じない）。
    pub(super) fn tail_text_within(&self, limit: Duration) -> String {
        // 何も送られないので、返るのは閉じたか期限かのどちらか。どちらでも末尾を読む。
        let _ = self.closed.recv_timeout(limit);

        let tail = self.tail.lock().unwrap_or_else(PoisonError::into_inner);
        let bytes: Vec<u8> = tail.iter().copied().collect();

        // 末尾を切った位置が文字の途中に来ることがある。そこだけ置換文字になる。
        String::from_utf8_lossy(&bytes).trim_end().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    /// テストで待つ期限。届くはずのものが届かないときだけ効く。
    const GENEROUS_LIMIT: Duration = Duration::from_secs(10);

    #[test]
    fn test_silence_limited_reader_passes_through_what_the_source_wrote() {
        let mut reader = SilenceLimitedReader::spawn(
            io::Cursor::new(b"Content-Length: 2".to_vec()),
            GENEROUS_LIMIT,
        )
        .expect("スレッドを作れる");

        let mut read = String::new();
        reader.read_to_string(&mut read).expect("尽きるまで読める");

        assert_eq!(read, "Content-Length: 2");
    }

    #[test]
    #[cfg(unix)]
    fn test_silence_limited_reader_on_a_source_that_stays_silent_times_out() {
        // 何も書かないまま生きている相手。期限が無ければ、ここで止まり続ける
        let mut silent = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::piped())
            .spawn()
            .expect("sleep を起動できる");
        let stdout = silent.stdout.take().expect("stdout をパイプにした");
        let mut reader = SilenceLimitedReader::spawn(stdout, Duration::from_millis(100))
            .expect("スレッドを作れる");

        let started = Instant::now();
        let error = reader
            .read(&mut [0; 16])
            .expect_err("何も届かないまま期限を過ぎる");

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < GENEROUS_LIMIT, "期限で戻る");

        let _ = silent.kill();
        let _ = silent.wait();
    }

    #[test]
    fn test_stderr_tail_keeps_only_the_last_bytes_of_a_long_output() {
        // 先頭の印は上限を超えて押し出され、末尾の印だけが残る
        let mut written = b"HEAD".to_vec();
        written.extend(std::iter::repeat_n(b'x', STDERR_TAIL_BYTES));
        written.extend(b"TAIL");

        let tail = StderrTail::spawn(io::Cursor::new(written))
            .expect("スレッドを作れる")
            .tail_text_within(GENEROUS_LIMIT);

        assert!(tail.ends_with("TAIL"), "末尾が残る");
        assert!(!tail.contains("HEAD"), "上限より前は残らない");
        assert_eq!(tail.len(), STDERR_TAIL_BYTES);
    }
}
