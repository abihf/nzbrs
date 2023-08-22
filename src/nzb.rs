use std::sync::Arc;

use serde_derive::Deserialize;

#[derive(Deserialize, Debug)]
pub struct Nzb {
	pub head: Head,
	#[serde(rename = "file")]
	pub files: Vec<Arc<File>>,
}

#[derive(Deserialize, Debug)]
pub struct Head {
	#[serde(rename = "meta")]
	pub metas: Vec<Meta>,
}

#[derive(Deserialize, Debug)]
pub struct Meta {
	#[serde(rename = "type")]
	pub key: String,

	#[serde(rename = "$value")]
	pub value: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct File {
	pub poster: String,
	pub subject: String,
	pub date: u32,

	pub groups: Groups,
	pub segments: Segments,
}

impl File {
	pub fn total_size(&self) -> u64 {
		self.segments.segment.iter().fold(0, |res, s| res + s.bytes)
	}

	pub fn name(&self) -> &str {
		let split: Vec<&str> = self.subject.splitn(3, '"').collect();
		split[1]
	}
}

#[derive(Deserialize, Debug, Clone)]
pub struct Groups {
	pub group: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Segments {
	pub segment: Vec<Arc<Segment>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Segment {
	pub bytes: u64,
	pub number: u32,
	#[serde(rename = "$value")]
	pub value: String,
}
