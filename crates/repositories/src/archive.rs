//! Bounded source archive generation and repository archive cache retention.
use gitadel_core::filesystem::open_private_file;
use sley::{ObjectId, Repository as GitRepository};
use sley_archive::{
    ArchiveConvert, ArchiveExtras, TarArchiveOptions, ZipArchiveOptions, write_tar_gz_archive_full,
    write_zip_archive_full,
};
use std::{
    fs::File,
    io::{self, Write},
    path::Path,
    time::{Duration as StdDuration, Instant},
};

/// Source archive encoding, independent of a request's query representation.
#[derive(Clone, Copy)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
}

const MAX_SOURCE_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const SOURCE_ARCHIVE_GENERATION_TIMEOUT: StdDuration = StdDuration::from_secs(2 * 60);
const MAX_CACHED_ARCHIVES_PER_REPOSITORY: usize = 8;

struct ArchiveFileWriter {
    file: File,
    bytes_written: u64,
    deadline: Instant,
}

impl Write for ArchiveFileWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "source archive generation timed out",
            ));
        }
        if self.bytes_written.saturating_add(bytes.len() as u64) > MAX_SOURCE_ARCHIVE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "source archive exceeds the output size limit",
            ));
        }
        let written = self.file.write(bytes)?;
        self.bytes_written = self.bytes_written.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

pub fn write_archive_file(
    repository_path: &Path,
    output_path: &Path,
    archive_format: ArchiveFormat,
    commit_oid: ObjectId,
    tree_oid: ObjectId,
    mtime: u64,
    prefix: String,
) -> anyhow::Result<()> {
    let repository = GitRepository::open_exact_bare(repository_path)?;
    let config = repository.config()?;
    let converter = ArchiveConvert::from_tree(
        repository_path,
        repository_path,
        &config,
        repository.object_database(),
        repository.object_format(),
        &tree_oid,
    )?;
    let mut writer = ArchiveFileWriter {
        file: open_private_file(output_path, true)?,
        bytes_written: 0,
        deadline: Instant::now() + SOURCE_ARCHIVE_GENERATION_TIMEOUT,
    };
    match archive_format {
        ArchiveFormat::Zip => write_zip_archive_full(
            &mut writer,
            repository.object_database(),
            repository.object_format(),
            &tree_oid,
            ZipArchiveOptions {
                prefix: prefix.into_bytes(),
                mtime,
                commit_id: Some(commit_oid),
                ..ZipArchiveOptions::default()
            },
            &converter,
            &ArchiveExtras::default(),
        )?,
        ArchiveFormat::TarGz => write_tar_gz_archive_full(
            &mut writer,
            repository.object_database(),
            repository.object_format(),
            &tree_oid,
            TarArchiveOptions {
                prefix: prefix.into_bytes(),
                mtime,
                commit_id: Some(commit_oid),
                ..TarArchiveOptions::default()
            },
            &converter,
            &ArchiveExtras::default(),
            6,
        )?,
    }
    writer.flush()?;
    writer.file.sync_all()?;
    Ok(())
}

pub fn prune_archive_cache(directory: &Path) -> anyhow::Result<()> {
    let mut entries = std::fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata
                .is_file()
                .then(|| (metadata.modified().ok(), entry.path()))
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.0);
    let remove = entries
        .len()
        .saturating_sub(MAX_CACHED_ARCHIVES_PER_REPOSITORY);
    for (_, path) in entries.into_iter().take(remove) {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Write as _,
        time::{Duration, Instant},
    };

    use uuid::Uuid;

    use super::{ArchiveFileWriter, MAX_SOURCE_ARCHIVE_BYTES};

    #[test]
    fn source_archive_writer_rejects_output_over_limit() {
        let path =
            std::env::temp_dir().join(format!("gitadel-archive-limit-test-{}", Uuid::new_v4()));
        let mut writer = ArchiveFileWriter {
            file: fs::File::create(&path).expect("create archive output"),
            bytes_written: MAX_SOURCE_ARCHIVE_BYTES,
            deadline: Instant::now() + Duration::from_secs(1),
        };

        let error = writer
            .write_all(&[0])
            .expect_err("reject oversized archive");
        fs::remove_file(path).expect("remove archive output");

        assert_eq!(error.kind(), std::io::ErrorKind::FileTooLarge);
    }

    #[test]
    fn source_archive_writer_rejects_expired_generation() {
        let path =
            std::env::temp_dir().join(format!("gitadel-archive-timeout-test-{}", Uuid::new_v4()));
        let mut writer = ArchiveFileWriter {
            file: fs::File::create(&path).expect("create archive output"),
            bytes_written: 0,
            deadline: Instant::now(),
        };

        let error = writer.write_all(&[0]).expect_err("reject expired archive");
        fs::remove_file(path).expect("remove archive output");

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }
}
