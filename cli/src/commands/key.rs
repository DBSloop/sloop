//! `sloop key export` and `sloop key import` — moving the one secret a restore needs.
//!
//! **The private key is the whole backup.** Losing it turns every encrypted dump into
//! noise, and no amount of disk redundancy helps, so the only real protection is a copy
//! somewhere sloop cannot reach: a password manager, a safe, a piece of paper. These two
//! commands exist to make that copy possible and to bring it back.
//!
//! **Export writes the key to standard output and nothing else to standard output.**
//! `sloop key export > backup-key.txt` produces a file with one line in it, and every word
//! of explanation goes to standard error where a redirect cannot pick it up. That is the
//! same shape `age-keygen` has, and it is what makes the command usable in a script.
//!
//! **Import reads it from standard input**, or asks for it without echoing when there is a
//! terminal. It refuses to replace a different key: the backups already taken are readable
//! only by the key they were written for, and quietly swapping it is how somebody discovers
//! in six months that half their archive is unopenable.

use std::io::{IsTerminal as _, Read as _, Write as _};
use std::path::Path;

use crate::crypt::{self, PrivateKey};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::file::{Encryption, KeyKept, SEALED_FILE};
use crate::registry::{Registries, Scope};
use crate::style;

/// Everything `key` needs from the outside.
pub struct Context<'a> {
    /// Both registries, already open. Written back when a key is created or replaced.
    pub registries: Registries,
    /// The global store, for the sentence that says which registry was read.
    pub global: &'a Path,
}

impl Context<'_> {
    /// The registry a key belongs to: the project's when there is one, else the global
    /// store — the same scope `db add` writes to.
    fn scope(&self) -> Scope {
        self.registries.writes_to()
    }

    /// Where that scope keeps its encrypted file.
    fn sealed(&self) -> std::path::PathBuf {
        self.registries
            .sealed_in(self.scope())
            .unwrap_or_else(|| self.global.join(SEALED_FILE))
    }

    /// The keypair this registry already has, if any.
    fn existing(&self) -> Option<Encryption> {
        self.registries
            .in_scope(self.scope())
            .and_then(|registry| registry.encryption())
            .cloned()
    }
}

/// Print the private key, creating the keypair if this registry has none yet.
///
/// Creating one here is deliberate: a person who runs `key export` is asking for a key to
/// keep, and refusing because there is nothing to export yet would be a riddle. It is the
/// same generation `backup` does, in the same place, with the same route.
pub fn export(context: &mut Context<'_>) -> Outcome<Exit> {
    let scope = context.scope();
    let sealed = context.sealed();

    let (encryption, fresh) = match context.existing() {
        Some(existing) => (existing, false),
        None => (create(&mut context.registries, scope, &sealed)?, true),
    };

    let private = crypt::Store {
        route: &encryption.private_key,
        sealed_file: &sealed,
    }
    .fetch(&encryption.public_key)?;

    // Everything about the key except the key, on the stream a redirect leaves alone.
    let mut explaining = anstream::stderr();
    let _ = writeln!(
        explaining,
        "{} {}",
        style::paint(if fresh {
            "a new backup key for"
        } else {
            "the backup key for"
        }),
        // The directory, and not `Resolution::describe`: that one appends *why* the registry
        // was chosen, which belongs in a sentence about resolution and reads as a non-sequitur
        // in a sentence about a key.
        style::dim(
            &context
                .registries
                .root_in(scope)
                .unwrap_or_else(|| context.global.to_path_buf())
                .display()
                .to_string()
        )
    );
    let _ = writeln!(
        explaining,
        "  {}",
        style::dim(&format!("public  {}", encryption.public_key))
    );
    let _ = writeln!(
        explaining,
        "  {}",
        style::dim(&format!(
            "private {}, and on standard output below",
            encryption.private_key.describe()
        ))
    );
    let _ = writeln!(
        explaining,
        "  {}",
        style::dim(
            "keep that line somewhere sloop cannot reach — a password manager, a safe. \
             Every encrypted backup this registry takes needs it, and nothing else can \
             replace it."
        )
    );

    // The key itself: one line, standard output, nothing around it.
    let mut out = std::io::stdout().lock();
    out.write_all(private.secret().expose().as_bytes())
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush())
        .map_err(|error| {
            Failure::new(Exit::Failure, format!("could not print the key: {error}"))
        })?;

    remember(&mut context.registries, scope, KeyKept::Exported)?;
    crate::report::result(serde_json::json!({ "exported": true, "registry": scope.label() }));
    Ok(Exit::Success)
}

/// Take a private key exported somewhere else.
pub fn import(context: &mut Context<'_>) -> Outcome<Exit> {
    let scope = context.scope();
    let sealed = context.sealed();
    let private = read_a_key()?;
    let public = private.public();

    if let Some(existing) = context.existing() {
        if existing.public_key == public {
            // Already the key this registry uses. Storing it again is harmless and is what
            // somebody moving to a new machine is actually doing.
            crypt::Store {
                route: &existing.private_key,
                sealed_file: &sealed,
            }
            .keep(&private)?;
            crate::say!(
                "{} {}",
                style::paint("already the key here"),
                style::dim(&public.to_string())
            );
            remember(&mut context.registries, scope, KeyKept::Exported)?;
            return Ok(Exit::Success);
        }

        return Err(Failure::new(
            Exit::Usage,
            format!(
                "this registry is already encrypting to {}, and that is a different key",
                existing.public_key
            ),
        )
        .hint(
            "every backup already taken can only be read with the key it was written for. \
             To move to a new key, take the `[encryption]` block out of the registry by \
             hand first — and keep the old key, or those backups are gone",
        ));
    }

    let route = crypt::keep_somewhere(&private, &sealed)?;
    write_the_block(
        &mut context.registries,
        scope,
        Encryption {
            public_key: public.clone(),
            private_key: route.clone(),
            // It came from somewhere else, so somewhere else has it.
            key_kept: Some(KeyKept::Exported),
        },
    )?;

    crate::say!(
        "{} {}",
        style::paint("imported"),
        style::dim(&public.to_string())
    );
    crate::say!(
        "  {}",
        style::dim(&format!(
            "kept in {}, and backups under {} are encrypted to it from now on",
            route.describe(),
            context
                .registries
                .root_in(scope)
                .unwrap_or_else(|| context.global.to_path_buf())
                .display()
        ))
    );
    crate::report::result(serde_json::json!({ "imported": true, "registry": scope.label() }));
    Ok(Exit::Success)
}

/// Generate a keypair for this registry and write it into the config.
///
/// Shared with `backup`, which reaches this same path the first time it runs against a
/// registry with no key.
pub fn create(registries: &mut Registries, scope: Scope, sealed: &Path) -> Outcome<Encryption> {
    let private = PrivateKey::generate();
    let route = crypt::keep_somewhere(&private, sealed)?;

    let encryption = Encryption {
        public_key: private.public(),
        private_key: route,
        // Nowhere but this machine yet, which is what makes the next backup stop and say so.
        key_kept: None,
    };

    write_the_block(registries, scope, encryption.clone())?;
    Ok(encryption)
}

/// Put the `[encryption]` block in the registry and save it.
fn write_the_block(
    registries: &mut Registries,
    scope: Scope,
    encryption: Encryption,
) -> Outcome<()> {
    registries.update(scope, |registry| {
        registry.set_encryption(encryption);
        Ok(())
    })
}

/// Record that the key has been written out, or that somebody declined to copy it.
///
/// `backup` calls this too: the question is asked where the first encrypted backup happens,
/// and the answer belongs in the same field `key export` writes.
pub fn remember(registries: &mut Registries, scope: Scope, kept: KeyKept) -> Outcome<()> {
    let Some(mut encryption) = registries
        .in_scope(scope)
        .and_then(|registry| registry.encryption())
        .cloned()
    else {
        return Ok(());
    };
    if encryption.key_kept == Some(kept) {
        return Ok(());
    }

    encryption.key_kept = Some(kept);
    write_the_block(registries, scope, encryption)
}

/// Read a private key from standard input, or ask for it at a terminal.
///
/// Piped input first, because that is what `sloop key import < backup-key.txt` does and
/// what a provisioning script does. A prompt only where there is a terminal to prompt at,
/// and it does not echo: a key pasted into a visible prompt is a key in the scrollback.
fn read_a_key() -> Outcome<PrivateKey> {
    if std::io::stdin().is_terminal() {
        let typed = rpassword::prompt_password("the exported key: ")
            .map_err(|error| Failure::usage(format!("could not read the key: {error}")))?;
        return PrivateKey::parse(&typed);
    }

    let mut piped = String::new();
    std::io::stdin()
        .read_to_string(&mut piped)
        .map_err(|error| {
            Failure::usage(format!(
                "could not read the key from standard input: {error}"
            ))
        })?;

    if piped.trim().is_empty() {
        return Err(
            Failure::new(Exit::Usage, "nothing arrived on standard input")
                .hint("`sloop key import < backup-key.txt`, or run it where there is a terminal"),
        );
    }

    PrivateKey::parse(&piped)
}
