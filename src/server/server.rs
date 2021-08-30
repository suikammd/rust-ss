use std::{
    convert::{TryFrom, TryInto},
    io::ErrorKind,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, ToSocketAddrs},
};

use crate::{codec::{Addr, codec::Command}, error::{CustomError, SocksResult}, util::copy::{client_read_from_tcp_to_websocket, client_read_from_websocket_to_tcp, server_read_from_tcp_to_websocket, server_read_from_websocket_to_tcp}};
use bytes::{BufMut, BytesMut};
use futures::{FutureExt, StreamExt, TryStreamExt};
use futures_util::future;
use tokio::{
    io::{copy_bidirectional, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpSocket, TcpStream},
};
use tokio_tungstenite::tungstenite::Message;

pub struct Server {
    listen_addr: String,
}

impl Server {
    pub fn new(listen_addr: String) -> Self {
        Self { listen_addr }
    }

    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        // TODO: change to websocket server
        let listener = TcpListener::bind(self.listen_addr).await?;
        while let Ok((inbound, addr)) = listener.accept().await {
            let serve = serve(inbound, addr).map(|r| {
                if let Err(e) = r {
                    println!("Failed to transfer; error={}", e);
                }
            });
            tokio::spawn(serve);
        }
        Ok(())
    }
}

async fn serve(inbound: TcpStream, addr: SocketAddr) -> Result<(), CustomError> {
    // parse connect packet
    let ws_stream = tokio_tungstenite::accept_async(inbound)
        .await
        .expect("Error during the websocket handshake occurred");

    // get connect addrs from connect packet
    let (mut input_write, mut input_read) = ws_stream.split();
    let addrs: Vec<SocketAddr>;
    match input_read.try_next().await {
        Ok(Some(msg)) => {
            let data = msg.into_data();
            addrs = Addr::from_bytes(data)?;
        }
        Ok(None) => {
            return Ok(());
        }
        Err(e) => {
            // TODO
            println!("{:?}", e);
            return Ok(());
        }
    }

    let mut target = TcpStream::connect(&addrs[..]).await?;
    println!("connect to proxy addrs successfully");
    let (mut output_read, mut output_write) = target.split();

    let (_, _) = tokio::join!(
        server_read_from_tcp_to_websocket(output_read, input_write),
        server_read_from_websocket_to_tcp(output_write, input_read)
    );
    Ok(())
}
