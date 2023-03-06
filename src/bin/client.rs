use std::env;

use nltt::{connect_to_game_server, protocol};
use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let game_server_port = env::var("GAME_SERVER_PORT").expect("GAME_SERVER_PORT environment variable not set");
    let game_server_port = game_server_port.parse::<u32>().expect("GAME_SERVER_PORT  environment variable is not a valid number");

    let signature = if let Ok(signature) = env::var("SIGNATURE") {
        Some(uuid::Uuid::parse_str(&signature).expect("uuid should be valid"))
    } else {
        None
    };

    let (mut client_reader, mut client_writer) =
        connect_to_game_server(&format!("127.0.0.1:{}", game_server_port), signature).await?;

    let mut send_content_timer = tokio::time::interval(std::time::Duration::from_secs(5));
    let (flash_sender, mut flash_receiver) = tokio::sync::mpsc::channel::<uuid::Uuid>(10);

    // Обработчик для Content фрейма, который нам будет присылать сервер
    tokio::spawn(async move {
        while let Some(Ok(frame)) = client_reader.read().await {
            match frame {
                protocol::PupaFrame::Content { msg_id, body } => {
                    flash_sender.send(msg_id).await.unwrap()
                }
                protocol::PupaFrame::Win { msg_id, body } => {
                    println!("User TOKEN is a winner for the message \"{}\"", msg_id);
                }
                _ => { /* Сервер не будет нам писать ничего кроме Content и Win, просто игнорируем */
                }
            }
        }
    });

    loop {
        tokio::select! {
            Some(msg_id) = flash_receiver.recv() => {
                println!("Sending the flash message: {}", msg_id);
                client_writer.write_flash(msg_id).await?;
            },
            _ = send_content_timer.tick() => {
                let uuid = uuid::Uuid::new_v4();
                println!("Writing regular content | msg_id: {}", uuid);
                client_writer.write_content(uuid, client_writer.generate_random_text()).await?;
            },
        }
    }

    Ok(())
}
