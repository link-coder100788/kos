//! Just enough TLS for https:// pages: TLS 1.3 with AES-128-GCM and P-256,
//! via the `embedded-tls` crate.
//!
//! The server's certificate chain must lead to one of the roots in `ROOTS_FILE`
//! and name the host we asked for.
//!
//! `embedded-tls` wants a socket it can block on, but the net task never
//! blocks; it steps every request along a little at a time. So TLS runs in
//! its own `tls` task, joined to the net task by a `Pipe`: two byte queues
//! of encrypted data. The net task shovels bytes between the queues and
//! the TCP socket; the tls task sleeps on `TLS_WAIT` whenever it needs
//! bytes that haven't arrived yet. Requests are handled one at a time.

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use embedded_io::ErrorKind;
use embedded_tls::{Certificate, CertificateEntryRef, CertificateRef, CertificateVerifyRef, CryptoProvider, TlsCipherSuite, TlsClock, TlsVerifier};
use embedded_tls::blocking::{Aes128GcmSha256, TlsConfig, TlsConnection, TlsContext, TlsError, UnsecureProvider};
use embedded_tls::pki::CertVerifier;
use rand_core::CryptoRngCore;
use smoltcp::socket::tcp;
use crate::arch::timer;
use crate::drivers::rtc::unix_time;
use crate::filesys::fs_ipc::{FsRequest, FsResponse, fs_call};
use crate::scheduler::wait::WaitQueue;
use crate::std::lock::irq_mutex::IrqMutex;

/// The largest possible TLS record (16 KiB of data plus overhead). Every
/// incoming record has to fit in the read buffer to be decrypted.
const RECORD_READ_BUF: usize = 16640;
const RECORD_WRITE_BUF: usize = 4096;

/// Wakes the tls task: a new job, or bytes from the server.
static TLS_WAIT: WaitQueue = WaitQueue::new();
static JOBS: IrqMutex<VecDeque<Job>> = IrqMutex::new(VecDeque::new());

/// The trusted root certificates: DER certificates one after another, so
/// they load in a single read.
const ROOTS_FILE: &str = "/.kos/certs/roots.der";
/// Room for the server's whole certificate chain, which the verifier keeps
/// a copy of (a full record, like RECORD_READ_BUF). 4 KiB was too small:
/// en.wikipedia.org and www.debian.org send over 4100 bytes, and the
/// error is embedded-tls's OutOfMemory. It lives on the tls task's
/// 256 KiB stack.
const CERT_SIZE: usize = 16384;

type RootVerifier = CertVerifier<'static, Aes128GcmSha256, SystemTlsClock, CERT_SIZE>;

struct SystemTlsClock;

impl TlsClock for SystemTlsClock {
    fn now() -> Option<u64> {
        unix_time()
    }
}

/// `CertVerifier` trusts exactly one root. This one walks up the server's
/// chain to the first certificate issued by one of our roots, then hands
/// the checking to a `CertVerifier` built for that root. Certificates above
/// that point are left out: servers often send extras, like their root
/// cross-signed by an older root we don't trust.
struct MultiRootVerifier {
    roots: &'static [Vec<u8>],
    host: Option<String>,
    chosen: Option<RootVerifier>,
}

impl TlsVerifier<Aes128GcmSha256> for MultiRootVerifier {
    fn set_hostname_verification(&mut self, hostname: &str) -> Result<(), TlsError> {
        self.host = Some(hostname.to_string());
        Ok(())
    }

    fn verify_certificate(
        &mut self,
        transcript: &<Aes128GcmSha256 as TlsCipherSuite>::Hash,
        mut cert: CertificateRef,
    ) -> Result<(), TlsError> {
        let mut found = None;
        for (i, entry) in cert.entries.iter().enumerate() {
            let CertificateEntryRef::X509(der) = entry else {
                return Err(TlsError::DecodeError);
            };
            let issuer = issuer_and_subject(der).ok_or(TlsError::DecodeError)?.0;
            let root = self.roots.iter()
                .find(|root| issuer_and_subject(root).is_some_and(|(_, subject)| subject == issuer));
            if let Some(root) = root {
                found = Some((i, root));
                break;
            }
        }
        let (top, root) = found.ok_or(TlsError::InvalidCertificate)?;
        cert.entries.truncate(top + 1);
        let mut verifier = RootVerifier::new(Certificate::X509(root.as_slice()));
        if let Some(host) = &self.host {
            verifier.set_hostname_verification(host)?;
        }
        verifier.verify_certificate(transcript, cert)?;
        self.chosen = Some(verifier);
        Ok(())
    }

    fn verify_signature(&mut self, verify: CertificateVerifyRef) -> Result<(), TlsError> {
        self.chosen.as_mut().ok_or(TlsError::InvalidCertificate)?.verify_signature(verify)
    }
}

/// Splits the first DER element off `buf`: (tag, whole element, contents, rest).
fn der_next(buf: &[u8]) -> Option<(u8, &[u8], &[u8], &[u8])> {
    let tag = *buf.first()?;
    let first = *buf.get(1)? as usize;
    let (len, header) = if first < 0x80 {
        (first, 2)
    } else {
        let n = first & 0x7f;
        if n == 0 || n > 4 {
            return None;
        }
        let len = buf.get(2..2 + n)?.iter().fold(0usize, |len, &b| len << 8 | b as usize);
        (len, 2 + n)
    };
    let end = header.checked_add(len)?;
    let whole = buf.get(..end)?;
    Some((tag, whole, &whole[header..], &buf[end..]))
}

/// The raw DER issuer and subject names of a certificate.
fn issuer_and_subject(cert: &[u8]) -> Option<(&[u8], &[u8])> {
    let (_, _, cert, _) = der_next(cert)?;
    let (_, _, tbs, _) = der_next(cert)?;
    // version is an optional [0]
    let mut rest = tbs;
    if rest.first() == Some(&0xa0) {
        rest = der_next(rest)?.3;
    }
    let rest = der_next(rest)?.3; // serial number
    let rest = der_next(rest)?.3; // signature algorithm
    let (_, issuer, _, rest) = der_next(rest)?;
    let rest = der_next(rest)?.3; // validity
    let (_, subject, _, _) = der_next(rest)?;
    Some((issuer, subject))
}

struct VerifiedProvider<R> {
    rng: R,
    verifier: MultiRootVerifier,
}

impl<R> VerifiedProvider<R> {
    fn new(rng: R, roots: &'static [Vec<u8>]) -> Self {
        Self { rng, verifier: MultiRootVerifier { roots, host: None, chosen: None } }
    }
}

/// Reads the root certificates in `ROOTS_FILE`.
fn load_roots() -> Vec<Vec<u8>> {
    let data = match fs_call(FsRequest::Read { cwd: "/".to_string(), path: ROOTS_FILE.to_string() }) {
        FsResponse::Data(data) => data,
        _ => {
            log::warn!("tls: can't read {}, so no https site will be trusted", ROOTS_FILE);
            return Vec::new();
        }
    };
    let mut roots = Vec::new();
    let mut rest = data.as_slice();
    while !rest.is_empty() {
        let Some((_, cert, _, next)) = der_next(rest) else {
            log::warn!("tls: {} is damaged after {} certificates", ROOTS_FILE, roots.len());
            break;
        };
        roots.push(cert.to_vec());
        rest = next;
    }
    log::debug!("tls: loaded {} root certificates", roots.len());
    roots
}

impl<R: CryptoRngCore> CryptoProvider for VerifiedProvider<R> {
    type CipherSuite = Aes128GcmSha256;
    type Signature = Vec<u8>;

    fn rng(&mut self) -> impl CryptoRngCore {
        &mut self.rng
    }

    fn verifier(&mut self) -> Result<&mut impl TlsVerifier<Self::CipherSuite>, TlsError> {
        Ok(&mut self.verifier)
    }
}

struct Job {
    host: String,
    request: Vec<u8>,
    pipe: Arc<Pipe>,
}

/// One TLS connection's link between the net task and the tls task.
pub struct Pipe(IrqMutex<PipeState>);

#[derive(Default)]
struct PipeState {
    /// Encrypted bytes from the tls task, waiting to go out on the socket.
    to_server: VecDeque<u8>,
    /// Encrypted bytes from the socket, waiting for the tls task.
    from_server: VecDeque<u8>,
    /// No more bytes will arrive: the server closed, or we gave up.
    server_closed: bool,
    /// The decrypted response, or why it failed, once the tls task is done.
    result: Option<Result<Vec<u8>, String>>,
}

/// Starts a TLS exchange over an already-connected socket: handshake, send
/// `request`, then read the response until the server closes.
pub fn start(host: &str, request: Vec<u8>) -> Arc<Pipe> {
    let pipe = Arc::new(Pipe(IrqMutex::new(PipeState::default())));
    JOBS.lock().push_back(Job { host: String::from(host), request, pipe: pipe.clone() });
    TLS_WAIT.wake_all();
    pipe
}

impl Pipe {
    /// Moves bytes between the socket and the queues. Called by the net
    /// task; returns the decrypted response once the exchange is over.
    pub fn pump(&self, socket: &mut tcp::Socket) -> Option<Result<Vec<u8>, String>> {
        let mut state = self.0.lock();
        while socket.can_send() && !state.to_server.is_empty() {
            let (front, _) = state.to_server.as_slices();
            match socket.send_slice(front) {
                Ok(0) | Err(_) => break,
                Ok(n) => { state.to_server.drain(..n); }
            }
        }
        let mut arrived = false;
        while socket.can_recv() {
            let mut chunk = [0u8; 2048];
            match socket.recv_slice(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    state.from_server.extend(&chunk[..n]);
                    arrived = true;
                }
            }
        }
        if !state.server_closed && (!socket.is_active() || !socket.may_recv()) {
            state.server_closed = true;
            arrived = true;
        }
        let result = state.result.take();
        drop(state);
        if arrived {
            TLS_WAIT.wake_all();
        }
        result
    }

    /// Gives up on the exchange (timeout); the tls task sees end-of-stream.
    pub fn abort(&self) {
        self.0.lock().server_closed = true;
        TLS_WAIT.wake_all();
    }
}

/// The tls task's end of a `Pipe`, in the form `embedded-tls` wants: a
/// socket you can read and write that blocks until it can.
struct PipeIo(Arc<Pipe>);

impl embedded_io::ErrorType for PipeIo {
    type Error = ErrorKind;
}

impl embedded_io::Read for PipeIo {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ErrorKind> {
        let mut n = 0;
        TLS_WAIT.wait_until(|| {
            let mut state = (self.0).0.lock();
            n = buf.len().min(state.from_server.len());
            for (dst, src) in buf.iter_mut().zip(state.from_server.drain(..n)) {
                *dst = src;
            }
            // Returning 0 bytes means end of stream.
            n > 0 || state.server_closed
        });
        Ok(n)
    }
}

impl embedded_io::Write for PipeIo {
    fn write(&mut self, buf: &[u8]) -> Result<usize, ErrorKind> {
        (self.0).0.lock().to_server.extend(buf);
        super::kick();
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<(), ErrorKind> {
        Ok(())
    }
}

fn exchange(job: &Job, roots: &'static [Vec<u8>], read_buf: &mut [u8], write_buf: &mut [u8]) -> Result<Vec<u8>, String> {
    let started = timer::uptime_ms();
    // Name the server (SNI), so a host serving many sites picks the right
    // one. We offer RSA signatures too: not checking them costs nothing.
    let config = TlsConfig::new().with_server_name(&job.host).enable_rsa_signatures();
    let mut tls: TlsConnection<_, Aes128GcmSha256> = TlsConnection::new(PipeIo(job.pipe.clone()), read_buf, write_buf);
    let provider = VerifiedProvider::new(crate::rand::Rng, roots);
    tls.open(TlsContext::new(&config, provider)).map_err(|e| { format!("TLS certificate verification/handshake with {} failed: {:?}", job.host, e) })?;
    log::debug!("tls: handshake with {} done in {} ms", job.host, timer::uptime_ms() - started);

    let mut sent = 0;
    while sent < job.request.len() {
        sent += tls.write(&job.request[sent..]).map_err(|e| format!("TLS write failed: {:?}", e))?;
    }
    tls.flush().map_err(|e| format!("TLS write failed: {:?}", e))?;

    let mut response = Vec::new();
    let mut chunk = vec![0u8; 4096];
    loop {
        match tls.read(&mut chunk) {
            Ok(0) | Err(TlsError::ConnectionClosed) => break,
            Ok(n) => {
                if response.len() + n <= super::MAX_RESPONSE {
                    response.extend_from_slice(&chunk[..n]);
                }
            }
            // Plenty of servers just drop the connection instead of saying
            // goodbye properly; once we have a response, that's fine.
            Err(_) if !response.is_empty() => break,
            Err(e) => return Err(format!("TLS read failed: {:?}", e)),
        }
    }
    log::debug!("tls: {} sent {} bytes in {} ms", job.host, response.len(), timer::uptime_ms() - started);
    Ok(response)
}

pub extern "C" fn tls_task() -> ! {
    // Loaded once and kept for good; the verifiers borrow from it.
    log::info!("tls: loading certs");
    let roots: &'static [Vec<u8>] = load_roots().leak();
    let mut read_buf = vec![0u8; RECORD_READ_BUF];
    let mut write_buf = vec![0u8; RECORD_WRITE_BUF];
    loop {
        let mut job = None;
        TLS_WAIT.wait_until(|| {
            job = JOBS.lock().pop_front();
            job.is_some()
        });
        let job = job.unwrap();
        let result = exchange(&job, roots, &mut read_buf, &mut write_buf);
        if let Err(e) = &result {
            log::warn!("{}", e);
        }
        job.pipe.0.lock().result = Some(result);
        super::kick();
    }
}
