pub mod protocol;

use futures::SinkExt;
use std::error::Error;
use tokio_stream::StreamExt;

// Read part of the client
pub struct ClientReader {
    stream: tokio_util::codec::FramedRead<tokio::net::tcp::OwnedReadHalf, protocol::PupaCodec>,
}

impl ClientReader {
    pub async fn read(&mut self) -> Option<Result<protocol::PupaFrame, std::io::Error>> {
        self.stream.next().await
    }
}

// Write part of the client
pub struct ClientWriter {
    stream: tokio_util::codec::FramedWrite<tokio::net::tcp::OwnedWriteHalf, protocol::PupaCodec>,
}

impl ClientWriter {
    pub async fn write_content(
        &mut self,
        msg_id: uuid::Uuid,
        body: Vec<u8>,
    ) -> Result<(), std::io::Error> {
        let frame = protocol::PupaFrame::Content { msg_id, body };

        self.stream.send(frame).await
    }

    pub async fn write_flash(&mut self, msg_id: uuid::Uuid) -> Result<(), std::io::Error> {
        let frame = protocol::PupaFrame::Flash { msg_id };

        self.stream.send(frame).await
    }

    // Подключившись к серверу клиент должен каждые 5 секунд отправлять
    // на сервер сообщение типа "КОНТЕНТ", формата {MSG_ID, BODY}.
    // MSG_ID должен быть уникальным (может быть UUID или другой). BODY
    // это рандомный текст размером от 30 до 100 байт.
    // Можно было бы добавить оверхеда и сделать std::String, но так как
    // мы особо не заботимся о структуре контента сообщения, а просто храним какие-то
    // байты, то пусть будет Vec<u8>. Если что, то легко поменять потом.
    pub fn generate_random_text(&self) -> Vec<u8> {
        use rand::Rng;

        let min_message_size = 30;
        let max_message_size = 100;
        let mut rng = rand::thread_rng();
        let size = rng.gen_range(min_message_size..max_message_size);

        let vec: Vec<u8> = rng
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(size)
            .collect();

        vec
    }
}

pub async fn connect_to_game_server(
    server_addr: &str,
) -> Result<(ClientReader, ClientWriter), Box<dyn Error>> {
    // let server_addr = "127.0.0.1:61616";
    println!("Connecting to {} ...", server_addr);

    let stream = tokio::net::TcpStream::connect(server_addr).await?;

    println!("Established connection to {}", server_addr);

    let codec = protocol::PupaCodec::new();
    let (read_half, write_half) = stream.into_split();

    let client_reader = ClientReader {
        stream: tokio_util::codec::FramedRead::new(read_half, codec.clone()),
    };

    let mut client_writer = ClientWriter {
        stream: tokio_util::codec::FramedWrite::new(write_half, codec),
    };

    println!("Authorizing with key provided");
    let frame = protocol::PupaFrame::Authorize {
        signature: uuid::Uuid::new_v4(),
    };
    let _ = client_writer.stream.send(frame).await;
    // TODO: обработать ошибку сети + ответ сервера в случае Unauth доступа (хотя имхо это не надо)

    Ok((client_reader, client_writer))
}
