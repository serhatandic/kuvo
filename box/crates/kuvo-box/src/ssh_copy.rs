use std::io::{self, BufRead, Read as _, Write as _};

use kuvo_setup::seed::{SshSeedCopy, SshSeedReport};
use kuvo_store::home::Home;

/// The copy command runs under the same account reservation as the SCP and join.
pub async fn apply(home: &Home) -> Result<SshSeedReport, kuvo_setup::Error> {
    let mut input = io::stdin().lock();
    let settings = read_phase(&mut input)
        .map_err(|source| failed(home, "reading the SSH copy for", source))?;
    let copy = SshSeedCopy::read(home, &settings)?;
    drop(settings);
    kuvo_keeper::provision_pinned_engine(home)
        .await
        .map_err(|source| failed(home, "provisioning the engine at", source))?;
    let keys = copy.settings()?;
    let engine =
        coordinator(home).map_err(|source| failed(home, "preparing the engine at", source))?;
    engine
        .with_install_engine(async {
            {
                let mut output = io::stdout().lock();
                output.write_all(b"{\"ready\":true}\n")?;
                output.flush()?;
            }
            let credentials = read_phase(&mut input)?;
            copy.import(keys, &credentials)
                .await
                .map_err(io::Error::other)
        })
        .await
        .map_err(|source| failed(home, "copying the selected sign-ins at", source))
}

fn read_phase(input: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input
        .take(SshSeedCopy::MAX_BYTES as u64 + 1)
        .read_until(b'\n', &mut bytes)?;
    if bytes.pop() != Some(b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "The SSH credential copy stopped before completing this step.",
        ));
    }
    Ok(bytes)
}

pub async fn rollback(home: &Home) -> Result<(), kuvo_setup::Error> {
    let copy = SshSeedCopy::resume_rollback(home)?;
    let engine =
        coordinator(home).map_err(|source| failed(home, "preparing the engine at", source))?;
    engine
        .stop_install_engine()
        .await
        .map_err(|source| failed(home, "stopping the staged engine at", source))?;
    copy.rollback()
}

fn coordinator(home: &Home) -> io::Result<kuvo_engine::Engine> {
    let env = std::env::vars_os()
        .filter(|(key, _)| key != "KUVO_SSH_ACCOUNT_TOKEN")
        .collect();
    kuvo_engine::Engine::new(home.root().to_owned(), env, kuvo_engine::Timing::default())
}

fn failed(home: &Home, action: &'static str, source: io::Error) -> kuvo_setup::Error {
    kuvo_setup::Error::File {
        action,
        path: home.root().into(),
        source,
    }
}
