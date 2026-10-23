// SPDX-License-Identifier: MIT
//! `fenix-tools release`: allowlisted binary ZIP and complete corresponding
//! source; no prefix traversal.
use crate::{
    Context,
    build::{read_lock, value},
    elf, licenses, util,
};
use fenix_patch::{
    Error, Result, VERSION, argv, core, fsx,
    json::{self, Dict, Value, json},
    process::{Cmd, Input, Output},
    py,
};
use flate2::{Compression, write::GzEncoder};
use regex::bytes::Regex;
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::LazyLock,
};
use zip::{CompressionMethod, DateTime, ZipWriter, write::SimpleFileOptions};

/// The files of the repository's top level that belong to its source.
pub const FILES: [&str; 10] = [
    "README.md",
    "LICENSE",
    "THIRD_PARTY.md",
    "BUILDING.md",
    "bundle.json",
    "install.sh",
    "Fenix A320 Linux Patch.desktop",
    ".gitignore",
    "Cargo.toml",
    "Cargo.lock",
];

/// The folders that belong to it with everything in them.
pub const FOLDERS: [&str; 8] = [
    "installer",
    "tools",
    "patches",
    "licenses",
    "tests",
    "docs",
    "native",
    ".github",
];

/// Names of people's own directories and tokens of their accounts.
static PRIVATE: LazyLock<[Regex; 2]> = LazyLock::new(|| {
    [
        r"(?-u)/home/[A-Za-z0-9_-]+/(?:Work|\.local/share/flightdeck/runtimes)",
        r"(?-u)XBL3[.]0 x=[0-9]+;[A-Za-z0-9_-]{30,}",
    ]
    .map(|pattern| Regex::new(pattern).unwrap_or_else(|error| unreachable!("{pattern}: {error}")))
});

/// Written in two parts, so that this file passes its own check.
const PRIVATE_KEY: &str = concat!("-----BEGIN ", "PRIVATE KEY-----");

const BLOCK: usize = 512;
/// Tar archives end on a multiple of this many bytes.
const RECORD: usize = 20 * BLOCK;

#[derive(Debug, Clone)]
pub struct Options {
    /// The prebuilt, statically linked `fenix-patch`.
    pub installer: PathBuf,
    /// `ROOT/dist` when not given.
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone)]
enum Content {
    File(PathBuf),
    Data(Vec<u8>),
}

/// One file of an archive, named relative to the archive's top-level folder.
#[derive(Debug, Clone)]
struct Member {
    name: PathBuf,
    content: Content,
    /// `None`: the permissions of the file itself.
    mode: Option<u32>,
}

impl Member {
    /// A file of the repository under its own name.
    fn own(root: &Path, file: &Path) -> Result<Self> {
        let name = file
            .strip_prefix(root)
            .map_err(|_| Error::other(format!("{} is outside the repository", fsx::text(file))))?;
        Ok(Self {
            name: name.to_path_buf(),
            content: Content::File(file.to_path_buf()),
            mode: None,
        })
    }

    /// Content and permission bits.
    fn load(&self) -> Result<(Vec<u8>, u32)> {
        Ok(match &self.content {
            Content::File(path) => (
                fsx::read(path)?,
                self.mode.unwrap_or(fsx::stat(path)?.mode()),
            ),
            Content::Data(data) => (data.clone(), self.mode.unwrap_or(0o644)),
        })
    }

    fn archive_name(&self, stem: &str) -> Result<String> {
        let name = self.name.to_str().ok_or_else(|| {
            Error::other(format!(
                "File name is not UTF-8: {}",
                self.name.to_string_lossy()
            ))
        })?;
        Ok(format!("{stem}/{name}"))
    }
}

fn is_regular(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|info| info.file_type().is_file())
}

/// Everything below `directory` that is a file. Build output and Python's
/// caches are left out; a symbolic link anywhere else is refused.
fn walk(directory: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    for path in fsx::sorted_dir(directory)? {
        let name = fsx::name(&path);
        if name == "__pycache__" || fsx::suffix(&name) == ".pyc" {
            continue;
        }
        let kind = fsx::lstat(&path)?.file_type();
        if kind.is_symlink() {
            return Err(value("Symlink in source export"));
        }
        if kind.is_file() {
            found.push(path);
        } else if kind.is_dir() && name != "target" {
            walk(&path, found)?;
        }
    }
    Ok(())
}

/// The allowlisted source files of the repository.
pub fn sources(root: &Path, lock: &Value) -> Result<Vec<PathBuf>> {
    let mut allowed: Vec<PathBuf> = FILES.iter().map(|name| root.join(name)).collect();
    for folder in FOLDERS {
        let folder = root.join(folder);
        match fs::symlink_metadata(&folder) {
            Ok(info) if info.file_type().is_symlink() => {
                return Err(value("Symlink in source export"));
            }
            Ok(info) if info.is_dir() => walk(&folder, &mut allowed)?,
            _ => {}
        }
    }
    for name in lock.need_table("integration")?.keys() {
        if !name.ends_with(".exe") {
            allowed.push(root.join("integration").join(name));
        }
    }
    Ok(allowed)
}

/// Every source file must be there, and none may give away where it was
/// written or carry a key or an account token.
pub fn check_sources(root: &Path, allowed: &[PathBuf]) -> Result<()> {
    for file in allowed {
        if !is_regular(file) {
            return Err(Error::Value(format!("Missing source: {}", fsx::text(file))));
        }
        let data = fsx::read(file)?;
        let key = PRIVATE_KEY.as_bytes();
        if PRIVATE.iter().any(|pattern| pattern.is_match(&data))
            || data.windows(key.len()).any(|part| part == key)
        {
            return Err(Error::Value(format!(
                "Private content in export: {}",
                fsx::text(file.strip_prefix(root).unwrap_or(file))
            )));
        }
    }
    Ok(())
}

/// The installer must run wherever the release is unpacked, and be the one of this version.
fn check_installer(ctx: &Context<'_>, path: &Path, version: &str) -> Result<Vec<u8>> {
    let refuse = |why: &str| Error::Value(format!("The installer {why}: {}", fsx::text(path)));
    if !fsx::lstat(path)?.file_type().is_file() {
        return Err(refuse("is not a regular file"));
    }
    let data = fsx::read(path)?;
    elf::check_static(&data).map_err(|refusal| refuse(&format!("is {refusal}")))?;
    let env = ctx.system.environ();
    let mut cmd = Cmd::new(argv![fsx::resolve(path)?, "--version"]);
    cmd.env = Some(&env);
    cmd.stdin = Input::Null;
    cmd.output = Output::Capture;
    cmd.timeout = Some(30);
    let finished = ctx.run(&cmd)?;
    let reported = String::from_utf8_lossy(&finished.stdout);
    if finished.code != 0 || py::strip(&reported) != format!("{} {version}", licenses::PACKAGE) {
        return Err(refuse(&format!("does not report version {version}")));
    }
    Ok(data)
}

/// A number as tar headers hold it: octal digits and a final NUL.
fn octal(field: &mut [u8], number: u64) {
    let digits = format!("{number:0width$o}", width = field.len() - 1);
    field[..digits.len()].copy_from_slice(digits.as_bytes());
}

/// One header block, as Python's `tarfile` writes it for the POSIX formats.
fn tar_header(name: &[u8], mode: u32, size: u64, kind: u8) -> [u8; BLOCK] {
    let mut block = [0u8; BLOCK];
    let length = name.len().min(100);
    block[..length].copy_from_slice(&name[..length]);
    octal(&mut block[100..108], u64::from(mode & 0o7777));
    // Owner and group 0 without names, modification time 0.
    octal(&mut block[108..116], 0);
    octal(&mut block[116..124], 0);
    octal(&mut block[124..136], size);
    octal(&mut block[136..148], 0);
    block[148..156].fill(b' ');
    block[156] = kind;
    block[257..265].copy_from_slice(b"ustar\x0000");
    let sum: u32 = block.iter().map(|byte| u32::from(*byte)).sum();
    block[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
    block
}

/// The extended header that carries a name the classic one cannot hold.
fn pax_path(name: &str) -> Vec<u8> {
    // The record starts with its own length, which includes that number.
    let rest = "path".len() + name.len() + 3;
    let mut length = 0;
    loop {
        let next = rest + length.to_string().len();
        if next == length {
            break;
        }
        length = next;
    }
    format!("{length} path={name}\n").into_bytes()
}

fn tar_entry(output: &mut impl Write, header: &[u8; BLOCK], data: &[u8]) -> Result<usize> {
    output.write_all(header)?;
    output.write_all(data)?;
    let padding = (BLOCK - data.len() % BLOCK) % BLOCK;
    output.write_all(&[0u8; BLOCK][..padding])?;
    Ok(BLOCK + data.len() + padding)
}

/// A gzip-compressed tar archive that depends on nothing but names, contents
/// and permissions: no owners, no times.
fn write_tar(path: &Path, stem: &str, members: &[Member]) -> Result<()> {
    let file = File::create(path).map_err(|error| Error::io_path(&error, path))?;
    let mut output = GzEncoder::new(BufWriter::new(file), Compression::new(9));
    let mut written = 0;
    for member in members {
        let name = member.archive_name(stem)?;
        let (data, mode) = member.load()?;
        if data.len() as u64 >= 1 << 33 {
            return Err(Error::Value(format!(
                "Too large for the source archive: {name}"
            )));
        }
        let mut short = name.clone().into_bytes();
        if !name.is_ascii() || name.len() > 100 {
            let record = pax_path(&name);
            let header = tar_header(b"././@PaxHeader", 0, record.len() as u64, b'x');
            written += tar_entry(&mut output, &header, &record)?;
            short = name
                .chars()
                .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
                .collect();
        }
        let header = tar_header(&short, mode, data.len() as u64, b'0');
        written += tar_entry(&mut output, &header, &data)?;
    }
    let end = 2 * BLOCK;
    let padding = (RECORD - (written + end) % RECORD) % RECORD;
    output.write_all(&vec![0u8; end + padding])?;
    output.finish()?.flush()?;
    Ok(())
}

fn zip_error(error: zip::result::ZipError) -> Error {
    Error::other(format!("The binary archive cannot be written: {error}"))
}

/// A ZIP archive with one fixed time for every file.
fn write_zip(path: &Path, stem: &str, members: &[Member]) -> Result<()> {
    let file = File::create(path).map_err(|error| Error::io_path(&error, path))?;
    let mut archive = ZipWriter::new(file);
    let time = DateTime::from_date_and_time(2026, 9, 23, 0, 0, 0)
        .map_err(|error| Error::other(error.to_string()))?;
    for member in members {
        let (data, mode) = member.load()?;
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(6))
            .last_modified_time(time)
            .unix_permissions(mode & 0o777)
            .large_file(data.len() as u64 >= u64::from(u32::MAX));
        archive
            .start_file(member.archive_name(stem)?, options)
            .map_err(zip_error)?;
        archive.write_all(&data)?;
    }
    archive.finish().map_err(zip_error)?.flush()?;
    Ok(())
}

/// Members in the order of their names, each name once.
fn ordered(mut members: Vec<Member>) -> Result<Vec<Member>> {
    members.sort_by(|left, right| left.name.cmp(&right.name));
    if let Some(pair) = members.windows(2).find(|pair| pair[0].name == pair[1].name) {
        return Err(Error::Value(format!(
            "Two files of the release have the same name: {}",
            fsx::text(&pair[0].name)
        )));
    }
    Ok(members)
}

pub fn run(ctx: &Context<'_>, options: &Options) -> Result<()> {
    let root = ctx.root();
    let lock = Value::Object(read_lock(root)?);
    if !json::is_text(lock.need("version")?, VERSION) {
        return Err(value("Installer and bundle versions differ"));
    }
    let installer = check_installer(ctx, &options.installer, VERSION)?;
    core::verify_bundle(ctx.system, root.as_os_str())?;
    let variants = lock.table("variants")?;
    let mut archives: Vec<PathBuf> = Vec::new();
    for entry in std::iter::once(&lock).chain(variants.values()) {
        for group in ["sources", "patches"] {
            for (name, expected) in &entry.need_table(group)? {
                if !json::is_text(expected, &fsx::digest(&root.join(group).join(name))?) {
                    return Err(Error::Value(format!("Source checksum mismatch: {name}")));
                }
            }
        }
        if let Some(dxvk) = entry.get("dxvk") {
            for (name, expected) in &dxvk.need_table("patches")? {
                if !json::is_text(expected, &fsx::digest(&root.join("patches").join(name))?) {
                    return Err(Error::Value(format!(
                        "DXVK patch checksum mismatch: {name}"
                    )));
                }
            }
        }
        for name in entry.need_table("sources")?.keys() {
            let archive = root.join("sources").join(name);
            if !archives.contains(&archive) {
                archives.push(archive);
            }
        }
    }
    let allowed = sources(root, &lock)?;
    check_sources(root, &allowed)?;
    // The source archive must hold the sources themselves, not a way to them.
    if let Some(link) = archives.iter().find(|archive| !is_regular(archive)) {
        return Err(Error::Value(format!("Missing source: {}", fsx::text(link))));
    }
    let notices = licenses::collect(&licenses::metadata(ctx)?)?;
    let mut source = Vec::new();
    for file in allowed.iter().chain(&archives) {
        source.push(Member::own(root, file)?);
    }
    let folder = Path::new(licenses::FOLDER);
    for text in &notices.texts {
        fsx::regular(&text.source, Some(4 * fsx::MIB))?;
        source.push(Member {
            name: folder.join(&text.name),
            content: Content::File(text.source.clone()),
            mode: Some(0o644),
        });
    }
    source.push(Member {
        name: folder.join("INVENTORY.json"),
        content: Content::Data((json::dumps(&notices.inventory, Some(2)) + "\n").into_bytes()),
        mode: Some(0o644),
    });
    let mut binary = source.clone();
    for name in lock.need_table("files")?.keys() {
        binary.push(Member::own(root, &root.join("payload").join(name))?);
    }
    for (variant, entry) in &variants {
        for name in entry.need_table("files")?.keys() {
            let file = root.join("payload/variants").join(variant).join(name);
            binary.push(Member::own(root, &file)?);
        }
    }
    for name in lock.need_table("integration")?.keys() {
        if name.ends_with(".exe") {
            binary.push(Member::own(root, &root.join("integration").join(name))?);
        }
    }
    binary.push(Member {
        name: PathBuf::from(licenses::PACKAGE),
        content: Content::Data(installer),
        mode: Some(0o755),
    });
    let (source, binary) = (ordered(source)?, ordered(binary)?);
    let output = options.output.clone().unwrap_or_else(|| root.join("dist"));
    util::mkdir_exist_ok(&output)?;
    let stem = format!("fenix-a320-linux-patch-{VERSION}");
    let source_path = output.join(format!("{stem}-source.tar.gz"));
    write_tar(&source_path, &stem, &source)?;
    let binary_path = output.join(format!("{stem}-linux-x86_64.zip"));
    write_zip(&binary_path, &stem, &binary)?;
    let outputs = [&binary_path, &source_path];
    let mut sums = String::new();
    for path in outputs {
        sums += &format!("{}  {}\n", fsx::digest(path)?, fsx::name(path));
    }
    util::write_text(&output.join("SHA256SUMS"), &sums)?;
    let release = json!({
        "version": VERSION,
        "url": format!(
            "{}/releases/download/v{VERSION}/{}",
            lock.need_str("repository")?,
            fsx::name(&binary_path)
        ),
        "sha256": fsx::digest(&binary_path)?,
        "archive_root": stem,
    });
    util::write_text(
        &output.join("flightdeck-release.json"),
        &(json::dumps(&release, Some(2)) + "\n"),
    )?;
    let mut update_release = release.clone();
    update_release["size"] = json!(fsx::stat(&binary_path)?.size());
    let update = json!({
        "format": 1,
        "installer_contract": 1,
        "minimum_flightdeck_version": "0.2.20",
        "release": update_release,
        "bundle": lock,
    });
    util::write_text(
        &output.join("flightdeck-update.json"),
        &(json::dumps(&update, Some(2)) + "\n"),
    )?;
    for path in outputs {
        ctx.print(&format!(
            "{} {} bytes",
            fsx::name(path),
            fsx::stat(path)?.size()
        ))?;
    }
    Ok(())
}
