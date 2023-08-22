mod downloader;
mod nntp;
mod nzb;
mod yenc;

use anyhow::Result;
use serde_xml_rs;
use std::{
	env, fs,
	sync::Arc,
	time::{Duration, SystemTime},
};

#[tokio::main]
async fn main() -> Result<()> {
	let str = fs::read_to_string("test.nzb")?;
	let desc: nzb::Nzb = serde_xml_rs::de::from_str(&str)?;

	let client = nntp::Client::new(nntp::Config {
		host: env::var("NZBRS_HOST").unwrap(),
		port: env::var("NZBRS_PORT").unwrap().parse().unwrap(),
		tls: true,
		user: env::var("NZBRS_USER").unwrap(),
		password: env::var("NZBRS_PASS").unwrap(),
		max_conn: 40,
	})
	.await;

	let mut dl = downloader::Downloader::new(Arc::new(client));
	// tokio::spawn(print_speed());
	dl.download(desc, "out").await?;
	Ok(())
}

const UNITS: [&str; 4] = ["bps", "kbps", "mbps", "gpbs"];

async fn print_speed() -> Result<()> {
	let mut interval = tokio::time::interval(Duration::from_secs(1));
	let now = SystemTime::now();
	let mut last_bytes = downloader::get_total_download();
	let mut last_time = now.elapsed()?;
	loop {
		interval.tick().await;
		let cur_bytes = downloader::get_total_download();
		let cur_time = now.elapsed()?;

		let delta_bytes = (cur_bytes - last_bytes) as f64;
		let delta_time = (cur_time - last_time).as_secs_f64();
		let mut speed = delta_bytes / delta_time;
		let mut unit_idx = 0;
		while speed > 1000.0 && unit_idx < 3 {
			speed /= 1000.0;
			unit_idx += 1;
		}
		println!("Speed {:.2} {}", speed, UNITS[unit_idx]);

		last_bytes = cur_bytes;
		last_time = cur_time;
	}
}
