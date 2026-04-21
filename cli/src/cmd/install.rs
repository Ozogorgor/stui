use anyhow::{Context, Result};

/// Install the plugin in the current directory.
///
/// With `--dev`, creates a symlink from `~/.stui/plugins/<name>` to the
/// current working directory.  The runtime's hot-reload watcher will pick it
/// up within ~500 ms if the runtime is already running.
///
/// Non-dev install (repository / registry) is reserved for a future release.
pub fn run(dev: bool) -> Result<()> {
    if !dev {
        anyhow::bail!(
            "--dev is required for now; non-dev install (registry / repository) \
             is a future release feature"
        );
    }

    let cwd = std::env::current_dir()?;
    let manifest_path = cwd.join("plugin.toml");
    if !manifest_path.exists() {
        anyhow::bail!("no plugin.toml in current directory");
    }

    let manifest: stui_plugin_sdk::PluginManifest = toml::from_str(
        &std::fs::read_to_string(&manifest_path).context("read plugin.toml")?,
    )
    .context("parse plugin.toml")?;

    let name = &manifest.plugin.name;

    // Guard against empty / path-traversal names.
    // manifest.plugin.name should be kebab-case ASCII but we validate
    // defensively here rather than trusting the manifest.
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name == "."
        || name == ".."
    {
        anyhow::bail!("invalid plugin.name in plugin.toml: {name:?}");
    }

    let home = dirs::home_dir().context("no home directory found")?;
    let plugins_dir = home.join(".stui").join("plugins");
    std::fs::create_dir_all(&plugins_dir)
        .with_context(|| format!("create {}", plugins_dir.display()))?;

    let target = plugins_dir.join(name);

    // Remove an existing entry (symlink) if present.  Refuse to overwrite a
    // real directory that we did not create — this prevents silently deleting
    // files the user copied in manually.
    if target.exists() || target.is_symlink() {
        if target.is_symlink() {
            std::fs::remove_file(&target)
                .with_context(|| format!("remove existing symlink {}", target.display()))?;
        } else {
            anyhow::bail!(
                "{} already exists and is not a symlink; refusing to overwrite. \
                 Remove it manually if you want to replace it.",
                target.display()
            );
        }
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(&cwd, &target)
        .with_context(|| format!("symlink {} → {}", cwd.display(), target.display()))?;

    #[cfg(not(unix))]
    anyhow::bail!("dev-mode symlink install is currently Unix-only");

    println!("Symlinked {} → {}", cwd.display(), target.display());
    println!(
        "Hot-reload watcher will pick it up within ~500ms (if the runtime is running)."
    );
    Ok(())
}
