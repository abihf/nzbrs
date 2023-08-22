use futures::stream::FuturesUnordered;
use futures::{Future, StreamExt};
use positioned_io::WriteAt;
use std::fs::File;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
	path::{Path, PathBuf},
	sync::Arc,
};
use tokio::sync::mpsc;
use tokio::task::{spawn_blocking, JoinHandle};

use crate::yenc::{FilePart, FileWriter};
use crate::{nntp, nzb};

static TOTAL_DOWNLOAD: AtomicUsize = AtomicUsize::new(0);

pub fn get_total_download() -> usize {
	TOTAL_DOWNLOAD.load(Ordering::Relaxed)
}

pub struct Downloader {
	client: Arc<nntp::Client>,
}

impl Downloader {
	pub(crate) fn new(client: Arc<nntp::Client>) -> Self {
		Self { client }
	}

	pub(crate) async fn download<P>(&mut self, desc: nzb::Nzb, folder: P) -> anyhow::Result<()>
	where
		P: AsRef<Path>,
	{
		wait_all(
			desc.files
				.iter()
				.filter(|file| file.name().ends_with(".mkv"))
				.map(|file| {
					let client = self.client.clone();
					let file_name = folder.as_ref().join(file.name());
					let file = file.clone();
					tokio::spawn(async move { download_file(client, file, file_name) })
				}),
		)
		.await?;
		Ok(())
	}
}

async fn wait_all<'a, I, Fut>(futures: I) -> anyhow::Result<()>
where
	I: Iterator<Item = JoinHandle<Fut>>,
	Fut: Future<Output = Result<(), anyhow::Error>>,
{
	let mut tasks = FuturesUnordered::new();
	for fut in futures {
		tasks.push(fut);
	}
	while let Some(task) = tasks.next().await {
		task?.await?;
	}
	Ok(())
}

async fn download_file(client: Arc<nntp::Client>, desc: Arc<nzb::File>, file_name: PathBuf) -> anyhow::Result<()> {
	let total_size = desc.total_size();
	// let file = spawn_blocking(move || -> anyhow::Result<File> {
	// 	let file = File::create(file_name)?;
	// 	file.set_len(total_size)?;
	// 	Ok(file)
	// })
	// .await??;

	// let file = Arc::new(Mutex::new(file));
	let mut offset: u64 = 0;
	let (tx, mut rx) = mpsc::channel(32);
	let mut tasks = FuturesUnordered::new();
	desc.segments.segment.iter().for_each(|segment| {
		let client = client.clone();
		let size = segment.bytes;
		let segment = segment.clone();
		let groups = desc.groups.group.clone();
		let tx = tx.clone();

		tasks.push(tokio::spawn(async move {
			download_segment(client, tx, segment, offset, groups)
		}));
		offset += size;
		// handler
	});

	spawn_blocking(move || -> anyhow::Result<()> {
		let mut file = File::create(file_name)?;
		file.set_len(total_size)?;

		while let Some(part) = rx.blocking_recv() {
			let size = part.buff.len();
			file.write_all_at(part.offset, &part.buff)?;
			TOTAL_DOWNLOAD.fetch_add(size, Ordering::Relaxed);
		}

		Ok(())
	});

	while let Some(task) = tasks.next().await {
		task?.await?;
	}

	Ok(())
}

async fn download_segment(
	client: Arc<nntp::Client>,
	tx: mpsc::Sender<FilePart>,
	desc: Arc<nzb::Segment>,
	offset: u64,
	groups: Vec<String>,
) -> anyhow::Result<()> {
	let mut conn = client.get().await.unwrap(); //.map_err(anyhow::Error::msg)?;

	for group in groups {
		if conn.set_group(group.as_str()).await.is_ok() {
			break;
		}
	}

	let c = FileWriter::new(tx, offset);
	conn.article_body(desc.value.as_str(), c).await?;
	Ok(())
}
