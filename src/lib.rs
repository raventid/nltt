// Здесь собранны те структуры, которы я использовал в сервере и клиенте
// пока их мало, я их просто определил в lib.rs

pub mod protocol;

use futures::SinkExt;
use linked_hash_map::LinkedHashMap;
use std::error::Error;
use tokio_stream::StreamExt;

// Реализация, которую использует клиент. Обертка над рид-стримом
pub struct ClientReader {
    stream: tokio_util::codec::FramedRead<tokio::net::tcp::OwnedReadHalf, protocol::PupaCodec>,
}

impl ClientReader {
    pub async fn read(&mut self) -> Option<Result<protocol::PupaFrame, std::io::Error>> {
        self.stream.next().await
    }
}

// Обертка над write-stream просто несколько удобных методов
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
    //
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
    signature: Option<uuid::Uuid>,
) -> Result<(ClientReader, ClientWriter, uuid::Uuid), Box<dyn Error>> {
    log::debug!("Connecting to {} ...", server_addr);

    let stream = tokio::net::TcpStream::connect(server_addr).await?;

    log::debug!("Established connection to {}", server_addr);

    let codec = protocol::PupaCodec::new();
    let (read_half, write_half) = stream.into_split();

    let client_reader = ClientReader {
        stream: tokio_util::codec::FramedRead::new(read_half, codec.clone()),
    };

    let mut client_writer = ClientWriter {
        stream: tokio_util::codec::FramedWrite::new(write_half, codec),
    };

    let signature = signature.unwrap_or_else(|| uuid::Uuid::new_v4());

    log::debug!("Authorizing with key provided {}", signature);

    let frame = protocol::PupaFrame::Authorize { signature };
    client_writer.stream.send(frame).await?;

    Ok((client_reader, client_writer, signature))
}

pub struct MessageStore {
    messages: linked_hash_map::LinkedHashMap<uuid::Uuid, Vec<u8>>,
}

impl MessageStore {
    pub fn new() -> Self {
        MessageStore {
            messages: LinkedHashMap::new(),
        }
    }

    // Наша модификация будет поддерживать размер хэшмапы в районе 500
    // Можно было сделать через чистилку в background треде, но я решил добавить
    // проверку прямо сюда. Придется брать lock() на всю очередь сообщений, когда
    // любому из клиентов захочется записать новый Content, еще придется брать lock()
    // когда мы будет искать победителя. Операций мало, lock хотя бы будет коротким.
    //
    // Еще один момент, мы считаем, что uuid всегда уникальные (это касается и ключей пользователя и msg_id)
    pub fn insert(&mut self, msg_id: uuid::Uuid, value: Vec<u8>) {
        if self.messages.len() == 500 {
            self.messages.pop_front();
        }

        // Если вдруг так получится, что у нас коллизия uuid, то мы просто затираем старое сообщение и даже не скажем об этом клиенту (но какова вероятность?)
        self.messages.insert(msg_id, value);
    }

    pub fn extract(&mut self, msg_id: uuid::Uuid) -> Option<(uuid::Uuid, Vec<u8>)> {
        self.messages.remove(&msg_id).map(|bytes| (msg_id, bytes))
    }
}

struct WinLog {
    signature: uuid::Uuid,
    timestamp: u64,
    msg_id: uuid::Uuid,
}

pub struct WinLogStore {
    records: std::collections::VecDeque<WinLog>,
}

impl WinLogStore {
    pub fn new() -> Self {
        WinLogStore {
            records: std::collections::VecDeque::new(),
        }
    }

    // Дата и время, Токен пользователя, MSG_ID
    pub fn insert(&mut self, msg_id: uuid::Uuid, signature: uuid::Uuid) {
        use std::time::SystemTime;
        let now = SystemTime::now();
        let timestamp = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("smth is wrong with time :D")
            .as_secs();

        if self.records.len() == 100 {
            self.records.pop_front();
        }

        // Если вдруг так получится, что у нас коллизия uuid, то мы просто затираем старое сообщение и даже не скажем об этом клиенту (но какова вероятность?)
        self.records.push_back(WinLog {
            msg_id,
            timestamp,
            signature,
        });
    }

    pub fn get_all(&self) -> Vec<(uuid::Uuid, u64, uuid::Uuid)> {
        self.records
            .iter()
            .map(|win_log| (win_log.signature, win_log.timestamp, win_log.msg_id))
            .collect()
    }
}
