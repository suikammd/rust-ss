use std::{
    convert::{TryFrom, TryInto},
    io::{ErrorKind, Read},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    sync::Arc,
};

use bytes::{BufMut, BytesMut};
use futures::{stream::SplitSink, FutureExt, SinkExt, StreamExt, TryFutureExt};
use tokio::{
    io::{self, copy_bidirectional, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter},
    net::{TcpListener, TcpStream, ToSocketAddrs},
};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use url::Url;

use crate::{
    codec::{
        Addr, {Command, RepCode},
    },
    error::{CustomError, SocksResult},
    util::copy::{client_read_from_tcp_to_websocket, client_read_from_websocket_to_tcp},
};

pub struct Client {
    listen_addr: String,
    server_url: Arc<Url>,
}

impl Client {
    pub fn new(listen_addr: String, proxy_addr: String) -> SocksResult<Self> {
        let server_url = url::Url::parse(format!("wss://{}", proxy_addr).as_str())?;
        Ok(Self {
            listen_addr,
            server_url: Arc::new(server_url),
        })
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(self.listen_addr.clone()).await?;
        println!("client listen on: {}", self.listen_addr);
        while let Ok((inbound, _)) = listener.accept().await {
            let serve = Client::serve(inbound, self.server_url.clone()).map(|r| {
                if let Err(e) = r {
                    println!("Failed to transfer; error={}", e);
                }
            });
            tokio::spawn(serve);
        }
        Ok(())
    }

    async fn serve(mut inbound: TcpStream, server_url: Arc<Url>) -> Result<(), CustomError> {
        println!("Get new connections");

        // socks5 handshake: decide which method to use
        let (cmd, addr) = Client::socks5_handshake(&mut inbound).await?;
        println!("cmd {:?} addr {:?}", cmd, addr);
        println!("handshake successfully");

        let (ws_stream, _) = connect_async(server_url.as_ref())
            .await
            .expect("Failed to connect");
        println!("WebSocket handshake has been successfully completed");

        let (input_read, input_write) = inbound.split();
        let (mut output_write, output_read) = ws_stream.split();
        let mut input_read = BufReader::new(input_read);
        // let mut input_write = BufWriter::new(input_write);

        // send connect packet
        let mut bytes = BytesMut::new();
        addr.to_bytes(&cmd, &mut bytes);
        output_write.send(Message::binary(bytes.to_vec())).await?;
        println!("send connect packet successfully");

        let (_, _) = tokio::join!(
            client_read_from_tcp_to_websocket(input_read, output_write),
            client_read_from_websocket_to_tcp(input_write, output_read)
        );

        Ok(())
    }

    async fn socks5_handshake(stream: &mut TcpStream) -> Result<(Command, Addr), CustomError> {
        let mut header = vec![0u8; 2];

        stream.read_exact(&mut header).await?;
        let socks_type = header[0];
        // validate socks type
        if socks_type != 0x05 {
            return Err(CustomError::UnsupportedSocksType(header[0]));
        }
        let method_len = header[1];
        let mut methods = vec![0u8; method_len as usize];
        stream.read_exact(&mut methods).await?;

        // validate methods
        // only support no auth for now
        let support = methods.into_iter().any(|method| method == 0);
        if !support {
            return Err(CustomError::UnsupportedMethodType);
        }

        stream.write_all(&[0x05, 0x00]).await?;
        let (cmd, addr) = Client::get_cmd_addr(stream).await?;
        stream.write_u8(0x05).await?;
        stream.write_u8(RepCode::Success.into()).await?;
        stream.write_u8(0x00).await?;
        addr.encode(stream).await?;
        Ok((cmd, addr))
    }

    async fn get_cmd_addr(stream: &mut TcpStream) -> SocksResult<(Command, Addr)> {
        let mut header = [0u8; 3];
        stream.read_exact(&mut header).await?;
        if header[0] != 0x05 {
            return Err(CustomError::UnsupportedSocksType(header[0]));
        }

        let cmd = Command::try_from(header[1])?;
        if cmd != Command::Connect {
            return Err(CustomError::UnsupportedCommand);
        }

        let addr = Addr::decode(stream).await?;
        println!("addr {:?}", addr);
        Ok((cmd, addr))
    }
}
