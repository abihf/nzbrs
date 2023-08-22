use anyhow::{Ok, Result};
use async_trait::async_trait;
use deadpool::managed::{Pool, RecycleResult};
use once_cell::sync::OnceCell;
use std::sync::Arc;
use tokio::{
	io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufStream},
	net::TcpStream,
};
use tokio_rustls::{client::TlsStream, rustls, TlsConnector};

#[derive(Clone, Default)]
pub struct Config {
	pub host: String,
	pub port: u16,
	pub user: String,
	pub password: String,
	pub tls: bool,

	pub max_conn: u32,
}

pub struct Client {
	pool: deadpool::managed::Pool<Manager>,
}

impl Client {
	pub async fn new(config: Config) -> Self {
		let max_conn = config.max_conn as usize;
		let manager = Manager {
			config,
			tls_conf: OnceCell::default(),
		};
		let pool = deadpool::managed::Pool::builder(manager)
			.max_size(max_conn)
			.build()
			.unwrap();

		Client { pool }
	}
}

impl std::ops::Deref for Client {
	type Target = Pool<Manager>;

	fn deref(&self) -> &Self::Target {
		&self.pool
	}
}

pub struct Connection {
	stream: Box<dyn Stream + Send>,
	buff: Vec<u8>,
	disconnected: bool,
	current_group: String,
}

impl Connection {
	async fn init(stream: Box<dyn Stream + Send>) -> Result<Self> {
		let mut conn = Self {
			stream,
			buff: Vec::with_capacity(256),
			disconnected: false,
			current_group: String::new(),
		};
		conn.read_response_line(200).await?;
		Ok(conn)
	}

	pub async fn article_body<C>(&mut self, name: &str, mut consumer: C) -> Result<()>
	where
		C: ArticleConsumer,
	{
		static DOTCRLF: &[u8] = ".\r\n".as_bytes();

		self.command(format!("BODY <{}>\r\n", name), 222).await?;
		loop {
			self.read_line().await?;
			if self.buff.eq(DOTCRLF) {
				break;
			}
			consumer.consume(self.buff.as_ref()).await?;
		}

		consumer.done().await?;
		Ok(())
	}

	pub async fn set_group(&mut self, group: &str) -> Result<()> {
		if !self.current_group.eq(group) {
			self.command(format!("GROUP {}\r\n", group), 211).await?;
			self.current_group = String::from(group);
		}
		Ok(())
	}

	// async fn close(&mut self) -> Result<()> {
	// 	self.stream.write_all("QUIT\r\n".as_bytes()).await
	// }

	async fn authenticate(&mut self, user: &str, password: &str) -> Result<()> {
		self.command(format!("AUTHINFO USER {}\r\n", user), 381).await?;
		self.command(format!("AUTHINFO PASS {}\r\n", password), 281).await?;

		Ok(())
	}

	async fn command<'a>(&mut self, cmd: String, expect_status: u16) -> Result<()> {
		if let Err(err) = self.buff.write_all(cmd.as_bytes()).await {
			self.disconnected = true;
			Err(err)?;
		}
		self.read_response_line(expect_status).await?;
		Ok(())
	}

	async fn read_line<'a>(&'a mut self) -> Result<()> {
		self.buff.clear();
		if let Err(err) = self.stream.read_line(&mut self.buff).await {
			self.disconnected = true;
			Err(err)?;
		};
		if self.buff.len() == 0 {
			self.disconnected = true;
			anyhow::bail!("disconnected")
		}
		Ok(())
	}

	async fn read_response_line(&mut self, expected: u16) -> Result<()> {
		self.read_line().await?;
		let str = String::from_utf8(self.buff.clone())?;
		let mut splitted = str.splitn(2, ' ');
		if let Some(status_str) = splitted.next() {
			let status: u16 = status_str.parse()?;
			if status != expected {
				anyhow::bail!("invalid status: exepect {} got {}", expected, status)
			}
			// let msg = splitted.next().unwrap_or_default();
			Ok(())
		} else {
			anyhow::bail!("invalid response")
		}
	}
}

#[async_trait]
pub trait ArticleConsumer {
	async fn consume(&mut self, line: &[u8]) -> Result<()>;
	async fn done(&mut self) -> Result<()>;
}

#[async_trait]
pub trait Stream {
	async fn read_line(&mut self, buf: &mut Vec<u8>) -> Result<usize>;
	async fn write_all(&mut self, buf: &[u8]) -> Result<()>;
}

pub struct Manager {
	pub config: Config,

	tls_conf: OnceCell<Arc<rustls::client::ClientConfig>>,
}

#[async_trait]
impl deadpool::managed::Manager for Manager {
	type Type = Connection;
	type Error = anyhow::Error;

	async fn create(&self) -> Result<Self::Type> {
		let stream = self.connect().await?;
		let mut conn = Connection::init(stream).await?;
		if let Err(err) = conn
			.authenticate(self.config.user.as_str(), self.config.password.as_str())
			.await
		{
			println!("{}", err);
		}
		Ok(conn)
	}

	async fn recycle(&self, conn: &mut Self::Type) -> RecycleResult<Self::Error> {
		if conn.disconnected {
			RecycleResult::Err(deadpool::managed::RecycleError::StaticMessage("disconnected"))
		// // Err(Error::Disconnected)
		// anyhow::bail!("disconnected")
		} else {
			RecycleResult::Ok(())
		}
	}

	fn detach(&self, _conn: &mut Self::Type) {
		// tokio::spawn(async move { conn.close().await });
	}
}

impl Manager {
	async fn connect(&self) -> Result<Box<dyn Stream + Send>> {
		if self.config.tls {
			Ok(Box::new(NntpStream {
				buf_stream: BufStream::new(self.connect_tls().await?),
			}))
		} else {
			Ok(Box::new(NntpStream {
				buf_stream: BufStream::new(self.connect_tcp().await?),
			}))
		}
	}

	async fn connect_tcp(&self) -> Result<TcpStream> {
		let host = self.config.host.as_str();
		let addr = (host, self.config.port);
		let stream = TcpStream::connect(addr).await?;
		Ok(stream)
	}

	async fn connect_tls(&self) -> Result<TlsStream<TcpStream>> {
		let tls_conf = self.tls_conf.get_or_init(|| {
			let mut root_store = rustls::RootCertStore::empty();
			root_store.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.iter().map(|ta| {
				rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(ta.subject, ta.spki, ta.name_constraints)
			}));
			let config = rustls::ClientConfig::builder()
				.with_safe_defaults()
				.with_root_certificates(root_store)
				.with_no_client_auth();
			Arc::new(config)
		});
		let connector = TlsConnector::from(tls_conf.clone());
		let host = self.config.host.as_str();
		let stream = self.connect_tcp().await?;
		let stream = connector.connect(host.try_into()?, stream).await?;
		Ok(stream)
	}
}

pub struct NntpStream<RW> {
	buf_stream: BufStream<RW>,
}

#[async_trait]
impl<RW> Stream for NntpStream<RW>
where
	RW: Send + AsyncRead + AsyncWrite + Unpin,
{
	async fn read_line(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
		let size = self.buf_stream.read_until(b'\n', buf).await?;
		Ok(size)
	}
	async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
		self.buf_stream.write_all(buf).await?;
		self.buf_stream.flush().await?;
		Ok(())
	}
}
