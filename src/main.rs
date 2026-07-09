// CodeCoder — autonomous AI agent. Entry shim; all wiring lives in lib.rs::run.
fn main() -> anyhow::Result<()> {
    codecoder::run(codecoder::Config::from_env())
}
