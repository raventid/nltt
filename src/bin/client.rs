use std::env;

use nltt::{connect_to_game_server, protocol};
use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    let game_server_port = env::var("GAME_SERVER_PORT")
        .expect("GAME_SERVER_PORT environment variable not set")
        .parse::<u32>()
        .expect("GAME_SERVER_PORT  environment variable is not a valid number");

    let signature = if let Ok(signature) = env::var("SIGNATURE") {
        Some(uuid::Uuid::parse_str(&signature).expect("uuid should be valid"))
    } else {
        None
    };

    let (mut client_reader, mut client_writer, signature) =
        connect_to_game_server(&format!("127.0.0.1:{}", game_server_port), signature).await?;

    let mut send_content_timer = tokio::time::interval(std::time::Duration::from_secs(5));
    let (flash_sender, mut flash_receiver) = tokio::sync::mpsc::channel::<uuid::Uuid>(10);

    tokio::spawn(async move {
        while let Some(Ok(frame)) = client_reader.read().await {
            match frame {
                protocol::PupaFrame::Content { msg_id, body: _ } => {
                    // Получив сообщение типа "КОНТЕНТ" от сервера, клиент должен
                    // запустить таймер на 1 секунду + random (от 250 до 500 ms). После
                    // истечения времени клиент посылает на сервер другое сообщение типа
                    // "ФЛЕШ", содержащее MSG_ID полученного сообщения.
                    let flash_sender = flash_sender.clone();
                    tokio::spawn(async move {
                        use rand::Rng;
                        let random = rand::thread_rng().gen_range(250..500);
                        tokio::time::sleep(std::time::Duration::from_millis(1_000 + random)).await;
                        flash_sender.send(msg_id).await.expect("flash channel should be alive")
                    });
                }
                protocol::PupaFrame::Win { msg_id, body } => {
                    log::info!(
                        "User {} is a winner for the message \"{}\"| message_body is {:?}",
                        signature,
                        msg_id,
                        body
                    );
                }
                _ => {
                    /* Сервер не будет нам писать ничего кроме Content и Win, просто игнорируем
                     * (Unauthorized мы тут не должны получить, он больше для несанкционнированых клиентов) */
                }
            }
        }
    });

    loop {
        tokio::select! {
            Some(msg_id) = flash_receiver.recv() => {
                log::debug!("Sending the flash message: {}", msg_id);
                client_writer.write_flash(msg_id).await?;
            },
            _ = send_content_timer.tick() => {
                let uuid = uuid::Uuid::new_v4();
                log::debug!("Writing regular content | msg_id: {}", uuid);
                client_writer.write_content(uuid, client_writer.generate_random_text()).await?;
            },
        }
    }
}
