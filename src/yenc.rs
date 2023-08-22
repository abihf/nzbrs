use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::nntp;

const NUL: u8 = 0;
//const TAB: u8 = b'\t';
const LF: u8 = b'\n';
const CR: u8 = b'\r';
// const SPACE: u8 = b' ';
const ESCAPE: u8 = b'=';
const DOT: u8 = b'.';
// const DEFAULT_LINE_SIZE: u8 = 128;

pub fn decode(encoded: &[u8], buff: &mut Vec<u8>) -> Result<()> {
	let mut iter = encoded.iter().cloned().enumerate();
	while let Some((col, byte)) = iter.next() {
		let mut result_byte = byte;
		match byte {
			NUL | CR | LF => {
				// for now, just continue
				continue;
			}
			DOT if col == 0 => match iter.next() {
				Some((_, DOT)) => {}
				Some((_, b)) => {
					buff.push(byte.overflowing_sub(42).0);
					result_byte = b;
				}
				None => {}
			},
			ESCAPE => {
				match iter.next() {
					Some((_, b)) => {
						result_byte = b.overflowing_sub(64).0;
					}
					None => {
						// for now, just continue
						continue;
					}
				}
			}
			_ => {}
		}
		buff.push(result_byte.overflowing_sub(42).0);
	}
	Ok(())
}

pub fn parse_command(cmd: &[u8]) -> anyhow::Result<HashMap<String, String>> {
	let mut map = HashMap::new();
	let str = String::from_utf8(cmd.to_vec())?;
	for part in str.trim().split(' ') {
		let name_val = part.split('=').collect::<Vec<&str>>();
		map.insert(name_val[0].to_string(), name_val[1].to_string());
	}
	Ok(map)
}

pub struct FilePart {
	pub offset: u64,
	pub buff: Vec<u8>,
}

pub struct FileWriter {
	offset: u64,
	tx: mpsc::Sender<FilePart>,
	has_header: bool,
	has_part: bool,
	hasher: crc32fast::Hasher,
}

impl FileWriter {
	pub fn new(tx: mpsc::Sender<FilePart>, offset: u64) -> Self {
		Self {
			tx,
			offset,
			has_header: false,
			has_part: false,
			hasher: crc32fast::Hasher::new(),
		}
	}
}

#[async_trait]
impl nntp::ArticleConsumer for FileWriter {
	async fn consume(&mut self, line: &[u8]) -> anyhow::Result<()> {
		if !self.has_header {
			if line.starts_with("=ybegin".as_bytes()) {
				self.has_header = true;
				return Ok(());
			} else {
				anyhow::bail!("invalid yenc. expect header")
			}
		}

		if !self.has_part {
			if line.starts_with("=ypart".as_bytes()) {
				let cmd = parse_command(&line[7..])?;
				if let Some(begin_str) = cmd.get("begin") {
					if let Ok(begin) = begin_str.parse::<u64>() {
						self.offset = begin - 1;
					}
				}
				self.has_part = true;
				return Ok(());
			} else {
				anyhow::bail!("invalid yenc. expect part")
			}
		}

		if line.starts_with("=yend".as_bytes()) {
			let cmd: std::collections::HashMap<String, String> = parse_command(&line[6..])?;
			if let Some(crc_str) = cmd.get("pcrc32") {
				let expected = u32::from_str_radix(&crc_str, 16)?;
				let actual = self.hasher.clone().finalize();
				if actual != expected {
					anyhow::bail!("invalid crc. expect {} got {}", expected, actual)
				}
			}
			return Ok(());
		}

		let mut buff = Vec::with_capacity(line.len());
		decode(line, &mut buff)?;
		self.hasher.update(&buff);

		let size = buff.len() as u64;
		let offset = self.offset;
		self.tx.send(FilePart { offset, buff }).await?;
		self.offset += size;
		Ok(())
	}

	async fn done(&mut self) -> anyhow::Result<()> {
		Ok(())
	}
}
