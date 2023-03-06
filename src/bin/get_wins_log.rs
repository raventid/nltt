use std::env;
use std::error::Error;

use futures::SinkExt;
use tokio_stream::StreamExt;

use nltt::{connect_to_game_server, protocol};


#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let api_server_port = env::var("API_SERVER_PORT").expect("API_SERVER_PORT environment variable not set");
    let api_server_port = api_server_port.parse::<u32>().expect("API_SERVER_PORT  environment variable is not a valid number");

    let server_addr = format!("127.0.0.1:{}", &api_server_port);

    println!("Connecting to {} ...", &server_addr);

   let stream = tokio::net::TcpStream::connect(server_addr.clone()).await?;
   let mut framed = tokio_util::codec::Framed::new(stream, protocol::PupaCodec::new());

   println!("Established connection to {}", server_addr);

    let frame = protocol::PupaFrame::ShowWinnersLog;

    framed.send(frame).await?;

    while let Some(result) = framed.next().await {
        match result {
            Ok(protocol::PupaFrame::WinLogRecord { signature, timestamp, msg_id }) => {
                println!("Signature: {}, timestamp: {}, msg_id: {}", signature, timestamp, msg_id );
            }
            _ => {
                // ignore
            }
        }
    }

    Ok(())
}
