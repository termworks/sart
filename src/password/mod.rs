//! Secure early-boot prompt primitives.
//!
//! Systemd and native coordinators are selectable explicitly by the daemon.
//! All exact adapters stay experimental until their encrypted-root VM gates
//! pass.

mod coordinator;
mod credential;
mod dracut_askpass;
mod input;
mod native_agent;
mod pipe_askpass;
mod secure;
mod systemd_agent;

pub use coordinator::{
    AskRequestSource, PromptCoordinator, SystemdPromptCoordinator, SystemdReplyTransport,
};
pub use credential::{
    CredentialPeerAuthenticator, LinuxCredentialPeerAuthenticator, MAX_RESPONDER_METADATA_BYTES,
    NativeCredentialClient, NativeCredentialError, NativeCredentialOutcome,
    NativeCredentialResponder, native_credential_pair, receive_responder, receive_responder_packet,
    send_responder, send_responder_packet,
};
pub use dracut_askpass::{DracutAskpassMetadata, DracutConsoleFallback};
pub use input::{EchoMode, InputFeedback, InputOutcome, InputRejection, PromptInput, PromptKey};
pub use native_agent::{
    NATIVE_ASKPASS_CANCELLED_EXIT_CODE, NATIVE_ASKPASS_OUTPUT_FD, NATIVE_ASKPASS_TIMEOUT,
    NATIVE_ASKPASS_TRANSPORT_EXIT_CODE, NativeAdapter, NativeAgentError,
    NativeAskpassClientOutcome, NativePromptCoordinator, claim_native_askpass_output,
    run_native_askpass_client, run_native_askpass_client_at,
};
pub use pipe_askpass::{
    PipeAskpassDisposition, PipeAskpassError, PipeAskpassMetadata, PipeSecretFraming,
    SAME_ELF_CLIENT, forward_ready_credential_to_pipe,
};
pub use secure::{
    DEFAULT_SECRET_BYTES, LinuxProcessSecretPolicy, MAX_SECRET_BYTES, ProcessSecretPolicy,
    SecretError, SecretProtection, SecureSecret,
};
pub use systemd_agent::{
    ASK_PASSWORD_DIRECTORY, AgentError, AskQueue, AskRequest, AskRequestId, CancellationReason,
    InotifyWatcher, LinuxRequesterLiveness, MAX_REQUEST_FILES, MonotonicClock, PromptDescriptor,
    QueueEvent, RejectedRequest, RequestDirectory, RequestWatcher, RequesterLiveness, ScanResult,
    SystemMonotonicClock, SystemdReplySocket, WatchBatch,
};
