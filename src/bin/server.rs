use std::{collections::HashMap, sync::Arc};

use futures::SinkExt;
use tokio::{sync::Mutex, select};
use tokio_stream::StreamExt;

use nltt::protocol;

// Наш лидерборд должен быть отсортирован по статусу пользователя online
// и числу побед.
//
// Как я понимаю эту формулировку - мы берем всех, кто онлайн и сортируем по одному фактору
// по числу побед.
struct Leaderboard {
    users: Vec<Peer>,
}

// может хранить пира под ключем, чтобы потом проще его искать было?
// или btree/hash set?
struct State {
    peers: HashMap<uuid::Uuid, Peer>,
}

impl State {
    fn new() -> Self {
        State {
            peers: HashMap::new(),
        }
    }

    // Попытаемся найти старого peer с таким же ключом, вдруг он уже у нас был
    // если был, то тогда заберем его старую статистику сюда
    pub fn add_peer(&mut self, peer: Peer) {
        self.peers.insert(peer.signature, peer);
    }

    // Мы просто отключим пира от канала для общения с его хэндлером
    // и поменяем статус на offline, а так пусть лежит в общей хэшмапе
    pub fn disable_peer(&mut self, signature: uuid::Uuid) {

    }

    async fn broadcast(&mut self, sender_signature: uuid::Uuid, message: protocol::PupaFrame) {
        for peer in self.peers.iter_mut() {
            if *peer.0 != sender_signature && peer.1.online == true {
                log::debug!("From {} Sending to {}, msg: {:?}", sender_signature, peer.1.signature, message);
                // TODO: Тут нужно подрефакторить clone(), если хватит времени
                peer.1
                    .channel
                    .clone()
                    .expect("we should only have active peers in broadcast")
                    .send(message.clone())
                    .await;
            }
        }
    }
}

#[derive(Debug)]
struct Peer {
    signature: uuid::Uuid,
    online: bool,
    messages_received: u32,
    messages_sent: u32,
    wins: u32,
    channel: Option<tokio::sync::mpsc::Sender<protocol::PupaFrame>>,
}

struct Message {
    msg_id: uuid::Uuid,
    body: Vec<u8>,
}

struct Messages {
    messages: Vec<String>,
}

// std::collections::HashMap<MSG_ID, LINK_TO_USER>

// Сервер должен иметь возможность обработать входящие сообщения
// типа "КОНТЕНТ" и сформировать из них лист/список.

// Каждое полученное сообщение с уникальным MSG_ID будет автоматически
// разослано всем клиентам.

// Получая сообщение с рандомным
// контентом от клиента бы должны обновить счётчик сообщений
// отправленных этим пользователем.

// Рассылая данное сообщение всем
// пользователям, мы должны обновить их счётчики полученных
// сообщений.

// Необходимо предусмотреть механизм очистки данного
// списка, чтобы автоматически из него удалялись сообщения при
// переполнении свыше 500 сообщений.

#[tokio::main]
async fn main() {
    env_logger::init();

    let port = 8000;
    let state = Arc::new(Mutex::new(State::new()));

    let game_server_listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port))
        .await
        .expect("Cannot bind listener to the port provided");

    // TODO: Для того чтобы реализовать API сделаем отдельный сервер,
    // который будет заниматься исключительно чтением статистики для клиентов.
    // let mut api_server_listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port))
    //       .await
    //       .expect("Cannot bind api_server_listener to the port provided");

    log::debug!("Started a game server at 127.0.0.1:{}", port);

    loop {
        let state = Arc::clone(&state);

        // В peer хранится ip адрес и порт входящего подключения.
        let (socket, peer) = game_server_listener.accept().await.unwrap();

        // Для каждого входящего подключения мы будем создавать отдельную задачу.
        tokio::spawn(async move {
            process(socket, peer, state).await;
        });
    }
}

async fn process(socket: tokio::net::TcpStream, peer: std::net::SocketAddr, state: Arc<Mutex<State>>) {
    log::debug!("New connection from {}:{}", peer.ip(), peer.port());

    let codec = protocol::PupaCodec::new();
    let (read_half, write_half) = socket.into_split();

    // Дуплексный канал для общения с клиентом по TCP
    let mut reader = tokio_util::codec::FramedRead::new(read_half, codec.clone());
    let mut writer = tokio_util::codec::FramedWrite::new(write_half, codec);

    // Внутренний канал, для обратной связи с модулем
    let (tx, mut rx) = tokio::sync::mpsc::channel::<protocol::PupaFrame>(10);
    let current_signature : uuid::Uuid;

    // Возможные вопросы по авторизации:
    // А что если клиент подключиться и не будет использовать подключение?
    // Я решил таймауты никуда не добавлять, но можно сделать какой-нибудь inactive_timeout, чтобы резать такие подключения.
    //
    // Получается, что все равно клиент может подключиться без авторизации и только после того, как пошлет первый фрейм он получит AuthError?
    // Да. Так как клиент получает клиентскую библиотеку для работы, то она инкапсулирует правильное поведение.
    // Если клиент пошлет плохой фрейм, мы отдаем Unauthorized, если клиент просто висит долго, то сейчас я ничего не делаю, но можно добавить таймаут.
    //
    if let Some(initial_bytes) = reader.next().await {
        match initial_bytes {
            Ok(protocol::PupaFrame::Authorize {
                signature,
            }) => {
                log::debug!("Authorizing peer [{}:{}]", peer.ip(), peer.port());
                // Окей, мы прошли авторизацию, можно добавить пользователя в наш список
                let new_peer = Peer {
                    signature: signature,
                    online: true,
                    messages_received: 0,
                    messages_sent: 0,
                    wins: 0,
                    channel: Some(tx),
                };

                state.lock().await.add_peer(new_peer);
                current_signature = signature;
            }
            Ok(_) => {
                log::debug!(
                    "Incorrect Authorization Frame | peer rejected [{}:{}]",
                    peer.ip(),
                    peer.port()
                );
                return;
            }
            Err(_) => {
                log::debug!(
                    "Malformed Authorization Frame | peer rejected [{}:{}]",
                    peer.ip(),
                    peer.port()
                );
                return;
            }
        }
    } else {
        log::debug!(
            "Socket disconneted on Authorization | peer rejected [{}:{}]",
            peer.ip(),
            peer.port()
        );
        return;
    }

    loop {
        tokio::select! {
            // Обработчик broadcast сообщений,
            // это до нас долетело чужое Content сообщение
            // Запишем его себе и клиент уже сам разеберется с Flash
            Some(msg) = rx.recv() => {
                writer.send(msg).await;
            }
            result = reader.next() => match result {
            Some(Ok(frame)) => match frame {
                // Если мы получили контент от клиента, то нам нужно разослать его всем активным
                // клиентам + сохранить сообщение в нашу коллекцию 500 последних сообщений
                protocol::PupaFrame::Content { msg_id, body } => {
                    log::debug!(
                        "Content | msg_id: {}, body: {:?} for [{}:{}] ",
                        msg_id,
                        body,
                        peer.ip(),
                        peer.port()
                    );

                    state.lock().await.broadcast(current_signature, protocol::PupaFrame::Content { msg_id, body }).await;
                }
                protocol::PupaFrame::Flash { msg_id } => {
                    log::debug!(
                        "Flash | msg_id: {} for [{}:{}]",
                        msg_id,
                        peer.ip(),
                        peer.port()
                    );
                }
                _ => {
                    // Нам могли заново отправить фрейм с авторизацией.
                    // В спецификации не указано, как на такое реагировать,
                    // поэтому мы просто проигнорируем такой фрейм в рамках
                    // сессии
                }
            },
            Some(Err(e)) => {
                log::error!("error on decoding from socket; error = {:?}", e);
            }
            _ => {/* None игнорируем */}
            },
        }
    }


    // Читаем фреймы, приходящие от клиента из сокета и обрабатываем, клиент у нас уже
    // авторизован, поэтому до закрытия сокета никакой авторизации для него требовать не будем
    // while let Some(result) = reader.next().await {
    //     match result {
    //         Ok(frame) => match frame {
    //             // Если мы получили контент, то нам нужно разослать его всем активным
    //             // клиентам + сохранить сообщение в нашу коллекцию 500 последних сообщений
    //             protocol::PupaFrame::Content { msg_id, body } => {
    //                 log::debug!(
    //                     "Content | msg_id: {}, body: {:?} for [{}:{}] ",
    //                     msg_id,
    //                     body,
    //                     peer.ip(),
    //                     peer.port()
    //                 );

    //                 state.lock().await.broadcast(current_signature, protocol::PupaFrame::Content { msg_id, body });
    //             }
    //             protocol::PupaFrame::Flash { msg_id } => {
    //                 log::debug!(
    //                     "Flash | msg_id: {} for [{}:{}]",
    //                     msg_id,
    //                     peer.ip(),
    //                     peer.port()
    //                 );
    //             }
    //             _ => {
    //                 // Нам могли заново отправить фрейм с авторизацией.
    //                 // В спецификации не указано, как на такое реагировать,
    //                 // поэтому мы просто проигнорируем такой фрейм в рамках
    //                 // сессии
    //             }
    //         },
    //         Err(e) => {
    //             log::error!("error on decoding from socket; error = {:?}", e);
    //         }
    //     }
    // }

    // Все, наш клиент отключился.
    // Поменяем ему статус на offline и отключим от канала.


    log::debug!("Peer disconnected [{}:{}]", peer.ip(), peer.port());
}
