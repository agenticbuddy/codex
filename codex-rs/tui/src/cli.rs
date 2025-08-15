use clap::Parser;
use codex_common::ApprovalModeCliArg;
use codex_common::CliConfigOverrides;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(version)]
pub struct Cli {
    /// Optional user prompt to start the session.
    pub prompt: Option<String>,

    /// Optional image(s) to attach to the initial prompt.
    #[arg(long = "image", short = 'i', value_name = "FILE", value_delimiter = ',', num_args = 1..)]
    pub images: Vec<PathBuf>,

    /// Model the agent should use.
    #[arg(long, short = 'm')]
    pub model: Option<String>,

    /// Convenience flag to select the local open source model provider.
    /// Equivalent to -c model_provider=oss; verifies a local Ollama server is
    /// running.
    #[arg(long = "oss", default_value_t = false)]
    pub oss: bool,

    /// Configuration profile from config.toml to specify default options.
    #[arg(long = "profile", short = 'p')]
    pub config_profile: Option<String>,

    /// Select the sandbox policy to use when executing model-generated shell
    /// commands.
    #[arg(long = "sandbox", short = 's')]
    pub sandbox_mode: Option<codex_common::SandboxModeCliArg>,

    /// Configure when the model requires human approval before executing a command.
    #[arg(long = "ask-for-approval", short = 'a')]
    pub approval_policy: Option<ApprovalModeCliArg>,

    /// Convenience alias for low-friction sandboxed automatic execution (-a on-failure, --sandbox workspace-write).
    #[arg(long = "full-auto", default_value_t = false)]
    pub full_auto: bool,

    /// Skip all confirmation prompts and execute commands without sandboxing.
    /// EXTREMELY DANGEROUS. Intended solely for running in environments that are externally sandboxed.
    #[arg(
        long = "dangerously-bypass-approvals-and-sandbox",
        alias = "yolo",
        default_value_t = false,
        conflicts_with_all = ["approval_policy", "full_auto"]
    )]
    pub dangerously_bypass_approvals_and_sandbox: bool,

    /// Tell the agent to use the specified directory as its working root.
    #[clap(long = "cd", short = 'C', value_name = "DIR")]
    pub cwd: Option<PathBuf>,

    #[clap(skip)]
    pub config_overrides: CliConfigOverrides,

    /// Browse previous sessions before starting (interactive selector).
    #[arg(long = "history", default_value_t = false)]
    pub history: bool,

    /// View a saved rollout file and exit (path to ~/.codex/sessions/*.json).
    #[arg(long = "view", value_name = "FILE")]
    pub view: Option<PathBuf>,

    /// Resume from a saved rollout file by injecting a prompt.
    #[arg(long = "resume", value_name = "FILE")]
    pub resume: Option<PathBuf>,

    /// Experimental: resume by loading the rollout on the backend (equivalent to -c experimental_resume=...)
    #[arg(long = "resume-experimental", value_name = "FILE")]
    pub resume_experimental: Option<PathBuf>,

    /// Attempt to resume using a provider/server conversation id (provider-specific; optional).
    #[arg(long = "resume-server", value_name = "TOKEN/ID")]
    pub resume_server: Option<String>,
}
