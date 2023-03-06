use std::{collections::HashMap, sync::Arc};

use futures::SinkExt;
use tokio::sync::Mutex;
use tokio_stream::StreamExt;

use nltt::protocol;
use nltt::MessageStore;
use nltt::WinLogStore;

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
        if let Some(previous_peer_record) = self.peers.get_mut(&peer.signature) {
            previous_peer_record.online = true;
            previous_peer_record.channel = peer.channel;
        } else {
            self.peers.insert(peer.signature, peer);
        }
    }

    // Мы просто отключим пира от канала для общения с его хэндлером
    // и поменяем статус на offline, а так пусть лежит в общей хэшмапе
    pub fn disable_peer(&mut self, signature: uuid::Uuid) {
        if let Some(active_peer) = self.peers.get_mut(&signature) {
            active_peer.online = false;
            active_peer.channel = None;
        }
    }

    pub fn update_winners(&mut self, signature: uuid::Uuid) {
        if let Some(active_peer) = self.peers.get_mut(&signature) {
            active_peer.wins += 1;
        }
    }

    // Не хочется менять Хэшмапу, потому что мы работаем с пользователем по ключу,
    // поэтому аллоцируем здоровый массив и сортируем его. Не очень быстро, но будем
    // надеятся, что не так часто сюда заходит клиент
    pub fn get_sorted_winners(&self) -> Vec<Peer> {
        let mut peers = self.peers.values().cloned().collect::<Vec<Peer>>();
        peers.sort_by(|a, b| a.online.cmp(&b.online).then_with(|| a.wins.cmp(&b.wins)));
        peers
    }

    // Когда я удаляю сообщения из MessageStore я не обновляю счетчики здесь,
    // то есть для меня это исторические счетчики и я никак не связываю их с сообщениями.
    // TODO: broadcast не очень дружит c single responsibilty
    async fn broadcast(&mut self, sender_signature: uuid::Uuid, message: protocol::PupaFrame) {
        // Обновим счетчик отправленых для sender
        if let Some(current_peer) = self.peers.get_mut(&sender_signature) {
            current_peer.messages_sent += 1;
        }

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

                peer.1.messages_received += 1;
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Peer {
    signature: uuid::Uuid,
    online: bool,
    messages_received: u32,
    messages_sent: u32,
    wins: u32,
    channel: Option<tokio::sync::mpsc::Sender<protocol::PupaFrame>>,
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let port = 8000;
    let api_port = 8010;

    let state = Arc::new(Mutex::new(State::new()));
    let message_store = Arc::new(Mutex::new(MessageStore::new()));
    let winlog_store = Arc::new(Mutex::new(WinLogStore::new()));

    // Этот сервер обрабатывает логику игры (общение с клиентами сообщения)
    let game_server_listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port))
        .await
        .expect("Cannot bind listener to the port provided");

    // Этот сервер обрабатывает АПИ запросы для статистики и так далее
    let mut api_server_listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", api_port))
          .await
          .expect("Cannot bind api_server_listener to the port provided");

    let game_state = Arc::clone(&state);
    let game_message_store = Arc::clone(&message_store);
    let game_winlog_store = Arc::clone(&winlog_store);

    // Запустим пару серверов на одном рантайме. Конечно с внешним хранилищем можно было бы разделить их на разные процессы.
    tokio::try_join!(
        tokio::spawn(async move {
     loop {

        let state = Arc::clone(&game_state);
        let message_store = Arc::clone(&game_message_store);
        let winlog_store = Arc::clone(&game_winlog_store);


        log::debug!("Started a game server at 127.0.0.1:{}", port);
        // В peer хранится ip адрес и порт входящего подключения.
        let (socket, peer) = game_server_listener.accept().await.unwrap();

        // Для каждого входящего подключения мы будем создавать отдельную задачу.
        // Можно было бы message_store положить в State, но у нас тогда была бы общая
        // write блокировка на добавляение новых peer и на запись сообщений в очередь.
        // Лучше разделим их, тем более это нам ничего не стоит.
        tokio::spawn(async move {
            run_game_handler(socket, peer, state, message_store, winlog_store).await;
        });
     } }),

tokio::spawn(async move {
     loop {
        let state = Arc::clone(&state);
        let message_store = Arc::clone(&message_store);
        let winlog_store = Arc::clone(&winlog_store);


        log::debug!("Started an API server at 127.0.0.1:{}", api_port);
        // В peer хранится ip адрес и порт входящего подключения.
        let (socket, peer) = api_server_listener.accept().await.unwrap();

        tokio::spawn(async move {
            run_api_handler(socket, peer, state, message_store, winlog_store).await;
        });
     } })
    );
}

async fn run_game_handler(socket: tokio::net::TcpStream, peer: std::net::SocketAddr, state: Arc<Mutex<State>>, message_store: Arc<Mutex<MessageStore>>, winlog_store: Arc<Mutex<WinLogStore>>) {
    log::debug!("New Game server connection from {}:{}", peer.ip(), peer.port());

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
            // Запишем его клиенту и пусть он готовит Flash
            Some(msg) = rx.recv() => {
                writer.send(msg).await;
            }
            // Здесь мы просто обрабатываем сокет от клиента
            // все, что он нам пишет приходит сюда
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

                    // Добавляем в список сообщений
                    message_store.lock().await.insert(msg_id, body.clone());
                    // Броадкастим на всех клиентов
                    state.lock().await.broadcast(current_signature, protocol::PupaFrame::Content { msg_id, body }).await;
                }
                protocol::PupaFrame::Flash { msg_id } => {
                    log::debug!(
                        "Flash | msg_id: {} for [{}:{}]",
                        msg_id,
                        peer.ip(),
                        peer.port()
                    );

                    // TODO: точно не помню, но кажется в такой семантике lock будет
                    // жить до конца if, а нам он нужен только чтобы быстро достать значения.
                    // Можно одной строчкой улучшить, но надо проверить.
                    if let Some((msg_id, body)) = message_store.lock().await.extract(msg_id) {
                        // А вот и наш победитель
                        state.lock().await.update_winners(current_signature);
                        winlog_store.lock().await.insert(msg_id, current_signature);
                        writer.send(protocol::PupaFrame::Win {msg_id, body}).await;
                    }
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
                _ => {
                    break;
                }
            },
        }
    }

    // Все, наш клиент отключился.
    // Поменяем ему статус на offline и отключим от канала.
    state.lock().await.disable_peer(current_signature);


    log::debug!("Peer disconnected [{}:{}]", peer.ip(), peer.port());
}

async fn run_api_handler(socket: tokio::net::TcpStream, peer: std::net::SocketAddr, state: Arc<Mutex<State>>, message_store: Arc<Mutex<MessageStore>>, winlog_store: Arc<Mutex<WinLogStore>>) {
    log::debug!("New API server connection from {}:{}", peer.ip(), peer.port());

    let codec = protocol::PupaCodec::new();
    let (read_half, write_half) = socket.into_split();

    // Дуплексный канал для общения с клиентом по TCP
    let mut reader = tokio_util::codec::FramedRead::new(read_half, codec.clone());
    let mut writer = tokio_util::codec::FramedWrite::new(write_half, codec);

    while let Some(result) = reader.next().await {
            match result {
                // Тут у нас запрашивают лог победителей
                Ok(protocol::PupaFrame::ShowWinners) => {
                    log::debug!(
                        "ShowWinnersLog | from [{}:{}] ",
                        peer.ip(),
                        peer.port()
                    );

                    let winners = state.lock().await.get_sorted_winners();
                    for record in winners.iter() {
                        writer.send(protocol::PupaFrame::WinnerRecord {
                            wins: record.wins,
                            messages_received: record.messages_received,
                            messages_sent: record.messages_sent
                        }).await;
                    }


                }
                // Тут у нас запрашивают лог побед
                Ok(protocol::PupaFrame::ShowWinnersLog) => {
                    log::debug!(
                        "ShowWinnersLog | from [{}:{}] ",
                        peer.ip(),
                        peer.port()
                    );

                    let records = winlog_store.lock().await.get_all();
                    for (signature, timestamp, msg_id) in records.into_iter() {
                        writer.send(protocol::PupaFrame::WinLogRecord {
                            signature,
                            timestamp,
                            msg_id
                        }).await;
                    }
                }
                Err(e) => {
                    log::error!("error on decoding from socket; error = {:?}", e);
                }
                _ => {
                    //ignore
                }
            }
    }

    // Все, наш клиент отключился.
    log::debug!("API Server | Peer disconnected [{}:{}]", peer.ip(), peer.port());
}
