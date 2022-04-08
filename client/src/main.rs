mod config;

use anyhow::{Context, Error};
use config::{Config, Server};
use input::EventWriter;
use net::{self, Message, PROTOCOL_VERSION};
use std::convert::Infallible;
use std::path::PathBuf;
use std::process;
use structopt::StructOpt;
use tokio::fs;
use tokio::io::BufReader;
use tokio::net::TcpStream;
use tokio::time;
use tokio_native_tls::native_tls::{Certificate, TlsConnector};
use futures::{future::select_all, FutureExt};

async fn try_connect(name: String, server: Server) -> Result<(String, Server, Certificate, TcpStream), Error> {
    let certificate = fs::read(&server.certificate_path).await
        .context("Failed to read certificate")?;

    let certificate = Certificate::from_der(&certificate)
        .or_else(|_| Certificate::from_pem(&certificate))
        .context("Failed to parse certificate")?;

    let (host, port) = (&server.server_address.host, server.server_address.port);

    log::info!("Attempting connection to {} ({}:{})", name, host, port);
    let stream = TcpStream::connect((host.as_str(), port)).await?;

    Ok((name, server, certificate, stream))
}

async fn run(mut config: Config) -> Result<Infallible, Error> {
    let (name, server, certificate, stream) = {
        let (res, _num, _vec) = select_all(config.drain().map(|(name, srv)| {
            try_connect(name.to_string(), srv).boxed()
        })).await;
        res?
    };

    let (host, port) = (&server.server_address.host, server.server_address.port);

    log::debug!("Connection open to {} ({}:{}), setting up TLS: {} ", name, host, port,
        server.certificate_path.to_string_lossy());

    let connector: tokio_native_tls::TlsConnector = TlsConnector::builder()
        .add_root_certificate(certificate)
        .build()
        .context("Failed to create connector")?
        .into();

    if let Err(err) = stream.set_nodelay(true) {
        log::warn!("setting TCP_NODELAY failed: {}", err);
    };

    let stream = BufReader::new(stream);
    let mut stream = connector
        .connect(&server.server_address.host, stream)
        .await
        .context("Failed to connect")?;

    log::info!("Connected to {} ({}:{})", name, host, port);

    net::write_version(&mut stream, PROTOCOL_VERSION).await?;

    let version = net::read_version(&mut stream).await?;
    if version != PROTOCOL_VERSION {
        return Err(anyhow::anyhow!(
            "Incompatible protocol version (got {}, expecting {})",
            version,
            PROTOCOL_VERSION
        ));
    }

    let mut writer = EventWriter::new().await?;
    loop {
        let message = time::timeout(net::MESSAGE_TIMEOUT, net::read_message(&mut stream))
            .await
            .context("Read timed out")??;
        match message {
            Message::Event(event) => writer.write(event).await?,
            Message::KeepAlive => {}
        }
    }
}

#[derive(StructOpt)]
#[structopt(name = "rkvm-client", about = "The rkvm client application")]
struct Args {
    #[structopt(help = "Path to configuration file")]
    #[cfg_attr(
        target_os = "linux",
        structopt(default_value = "/etc/rkvm/client.toml")
    )]
    #[cfg_attr(
        target_os = "windows",
        structopt(default_value = "C:/rkvm/client.toml")
    )]
    config_path: PathBuf,
}

#[tokio::main]
async fn main() {
    env_logger::builder()
        .format_timestamp(None)
        .init();

    let args = Args::from_args();
    let config = match fs::read_to_string(&args.config_path).await {
        Ok(config) => config,
        Err(err) => {
            log::error!("Error loading config: {}", err);
            process::exit(1);
        }
    };

    let config: Config = match toml::from_str(&config) {
        Ok(config) => config,
        Err(err) => {
            log::error!("Error parsing config: {}", err);
            process::exit(1);
        }
    };

    tokio::select! {
        result = run(config.clone()) => {
            if let Err(err) = result {
                log::error!("Error: {:#}", err);
                process::exit(1);
            }
        }
        result = tokio::signal::ctrl_c() => {
            if let Err(err) = result {
                log::error!("Error setting up signal handler: {}", err);
                process::exit(1);
            }

            log::info!("Exiting on signal");
        }
    }
}
