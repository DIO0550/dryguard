//! 子プロセスの入力へ別スレッドで書き込む。書く側は期限付きで書き終わりを待つ。
//!
//! **スレッドは子プロセスの境界にだけ置く。** [`super::connection::Connection`] は
//! `Write` を受けるだけで、書き口が期限を持つかを知らない
//! （[`super::child_output`] と同じ形。rules/tdd.md「`lsp` は『応答を受け取ってから先』を切り出す」）。

use std::io::{self, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::Duration;

/// 1 度に書き込みのスレッドへ渡す量の上限。
///
/// **パイプのバッファ（Linux で 64 KiB）より小さく取る。** 期限は 1 塊ごとに数えるので、
/// 塊がバッファより大きいと、読んでいる相手でも 1 塊を受け取り終えるまでに時間がかかる。
const WRITE_CHUNK_BYTES: usize = 8 * 1024;

/// 別スレッドへ書かせ、書き終わりを期限付きで待つ書き口。
///
/// **期限は 1 塊が受け取られないまま待つ時間で数える。** 1 回の書き込みは
/// [`WRITE_CHUNK_BYTES`] までしか渡さず、`write_all` がそれを繰り返す。大きなドキュメントでも、
/// 相手が読み続けている限り切られない。
///
/// **Why not（書き終わりを待たずに投げる）**: 書いた時点で `BrokenPipe` が返らなくなり、
/// 握手の失敗を「接続を閉じた」と見分けられなくなる。stdin を読まないままログを流し続ける
/// 相手では、読み側の沈黙の上限も効かない。
#[derive(Debug)]
pub(super) struct IntakeLimitedWriter {
    chunks: SyncSender<Vec<u8>>,
    written: Receiver<io::Result<()>>,
    intake_limit: Duration,
    /// 期限を超えたか。超えた後のスレッドは止まった塊を握ったままなので、次を渡すと詰まる。
    exceeded: bool,
}

impl IntakeLimitedWriter {
    /// `destination` へ別スレッドで書き、1 塊が `intake_limit` を超えて受け取られなければ
    /// 書き込みを [`io::ErrorKind::TimedOut`] で失敗させる書き口を作る。
    ///
    /// スレッドは書き口が落ちるか、書き込みが失敗すると終わり、`destination` を閉じる。
    /// 止まったまま待っている書き込みは、子プロセスを kill すれば失敗して終わる。
    /// サーバが起こした孫プロセスが入力を握っていると終わらずに残る（join しないので、
    /// こちらが止まることはない。[`super::child_output`] の吸う側と同じ）。
    ///
    /// # Errors
    ///
    /// 書くスレッドを作れないとき。
    pub(super) fn spawn<W: Write + Send + 'static>(
        destination: W,
        intake_limit: Duration,
    ) -> io::Result<Self> {
        let (chunks, received) = mpsc::sync_channel::<Vec<u8>>(1);
        let (finished, written) = mpsc::sync_channel(1);

        thread::Builder::new().spawn(move || {
            let mut destination = destination;

            for chunk in received {
                let outcome = destination
                    .write_all(&chunk)
                    .and_then(|()| destination.flush());
                let failed = outcome.is_err();

                // 受け手が居なくなったら、書き続けても知らせる先が無い。
                if finished.send(outcome).is_err() || failed {
                    return;
                }
            }
        })?;

        Ok(Self {
            chunks,
            written,
            intake_limit,
            exceeded: false,
        })
    }

    fn intake_exceeded_error(&self) -> io::Error {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "{} 秒のあいだ受け取られませんでした",
                self.intake_limit.as_secs_f64()
            ),
        )
    }
}

impl Write for IntakeLimitedWriter {
    /// # Errors
    ///
    /// 期限を超えて受け取られなかったとき（[`io::ErrorKind::TimedOut`]。以後は待たずに同じ種類で
    /// 断る）、書き込み自体が失敗したとき（相手が閉じていれば [`io::ErrorKind::BrokenPipe`]）。
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.exceeded {
            return Err(self.intake_exceeded_error());
        }

        if buffer.is_empty() {
            return Ok(0);
        }

        let chunk = &buffer[..buffer.len().min(WRITE_CHUNK_BYTES)];

        // スレッドが終わっているのは、前の書き込みが失敗したとき。その理由は返し済み。
        if self.chunks.send(chunk.to_vec()).is_err() {
            return Err(io::ErrorKind::BrokenPipe.into());
        }

        match self.written.recv_timeout(self.intake_limit) {
            Ok(Ok(())) => Ok(chunk.len()),
            Ok(Err(cause)) => Err(cause),
            Err(RecvTimeoutError::Timeout) => {
                self.exceeded = true;
                Err(self.intake_exceeded_error())
            }
            Err(RecvTimeoutError::Disconnected) => Err(io::ErrorKind::BrokenPipe.into()),
        }
    }

    /// スレッドは塊ごとに書き出してから知らせるので、溜めているものは無い。
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    /// テストで待つ期限。届くはずのものが届かないときだけ効く。
    const GENEROUS_LIMIT: Duration = Duration::from_secs(10);

    /// 読まない相手に書いて、期限に触れさせるための期限。
    const SHORT_LIMIT: Duration = Duration::from_millis(300);

    /// パイプのバッファを超える量。読まない相手に書くと、途中で止まる。
    ///
    /// Linux の既定のバッファは 16 ページで、x86_64 では 64 KiB、64 KiB ページの
    /// カーネルでは 1 MiB。どちらも超えるよう広く取る。
    const MORE_THAN_PIPE_BUFFER: usize = 4 * 1024 * 1024;

    #[test]
    #[cfg(unix)]
    fn test_intake_limited_writer_delivers_what_was_written() {
        let mut echo = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("cat を起動できる");
        let stdin = echo.stdin.take().expect("stdin をパイプにした");
        let mut stdout = echo.stdout.take().expect("stdout をパイプにした");
        let mut writer =
            IntakeLimitedWriter::spawn(stdin, GENEROUS_LIMIT).expect("スレッドを作れる");

        writer.write_all(b"Content-Length: 2").expect("書ける");
        // 書き口を落とすと stdin が閉じ、cat が読み終えて stdout を閉じる
        drop(writer);
        let mut echoed = String::new();
        stdout
            .read_to_string(&mut echoed)
            .expect("尽きるまで読める");

        assert_eq!(echoed, "Content-Length: 2");
        let _ = echo.wait();
    }

    #[test]
    #[cfg(unix)]
    fn test_intake_limited_writer_to_a_reading_peer_delivers_more_than_the_pipe_buffer() {
        // 対照は下のテスト。読み続ける相手には、パイプのバッファを超える量も塊に分けて全部届く
        let mut counter = Command::new("wc")
            .arg("-c")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("wc を起動できる");
        let stdin = counter.stdin.take().expect("stdin をパイプにした");
        let mut stdout = counter.stdout.take().expect("stdout をパイプにした");
        let mut writer = IntakeLimitedWriter::spawn(stdin, SHORT_LIMIT).expect("スレッドを作れる");

        writer
            .write_all(&vec![b'x'; MORE_THAN_PIPE_BUFFER])
            .expect("読み続ける相手には書き切れる");
        drop(writer);
        let mut counted = String::new();
        stdout
            .read_to_string(&mut counted)
            .expect("尽きるまで読める");

        assert_eq!(counted.trim(), MORE_THAN_PIPE_BUFFER.to_string());
        let _ = counter.wait();
    }

    #[test]
    #[cfg(unix)]
    fn test_intake_limited_writer_to_a_peer_that_does_not_read_times_out() {
        // stdin を読まないまま生きている相手。期限が無ければ、パイプが埋まってここで止まり続ける
        let mut stalled = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::piped())
            .spawn()
            .expect("sleep を起動できる");
        let stdin = stalled.stdin.take().expect("stdin をパイプにした");
        let mut writer = IntakeLimitedWriter::spawn(stdin, SHORT_LIMIT).expect("スレッドを作れる");

        let started = Instant::now();
        let error = writer
            .write_all(&vec![b'x'; MORE_THAN_PIPE_BUFFER])
            .expect_err("受け取られないまま期限を過ぎる");

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < GENEROUS_LIMIT, "期限で戻る");

        let _ = stalled.kill();
        let _ = stalled.wait();
    }

    #[test]
    #[cfg(unix)]
    fn test_intake_limited_writer_after_timing_out_refuses_without_waiting_again() {
        // 書き込みのスレッドは止まった塊を握ったまま。次を渡すと、そこで詰まる
        let mut stalled = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::piped())
            .spawn()
            .expect("sleep を起動できる");
        let stdin = stalled.stdin.take().expect("stdin をパイプにした");
        let mut writer = IntakeLimitedWriter::spawn(stdin, SHORT_LIMIT).expect("スレッドを作れる");
        let _ = writer.write_all(&vec![b'x'; MORE_THAN_PIPE_BUFFER]);

        let started = Instant::now();
        let error = writer.write(b"x").expect_err("待たずに断る");

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < SHORT_LIMIT, "2 度目は待たない");

        let _ = stalled.kill();
        let _ = stalled.wait();
    }

    #[test]
    #[cfg(unix)]
    fn test_intake_limited_writer_to_a_closed_peer_reports_a_broken_pipe() {
        // 握手の失敗を「接続を閉じた」と見分けるのに、`BrokenPipe` の種類が要る
        let mut exited = Command::new("true")
            .stdin(Stdio::piped())
            .spawn()
            .expect("true を起動できる");
        let stdin = exited.stdin.take().expect("stdin をパイプにした");
        let _ = exited.wait();
        let mut writer =
            IntakeLimitedWriter::spawn(stdin, GENEROUS_LIMIT).expect("スレッドを作れる");

        let error = writer.write_all(b"x").expect_err("閉じた相手には書けない");

        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }
}
