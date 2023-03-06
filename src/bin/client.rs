use nltt::{connect_to_game_server, protocol, ClientReader, ClientWriter};
use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let port = 8000;
    let (mut client_reader, mut client_writer) =
        connect_to_game_server(&format!("127.0.0.1:{}", port)).await?;

    let mut send_content_timer = tokio::time::interval(std::time::Duration::from_secs(5));
    let (flash_sender, mut flash_receiver) = tokio::sync::mpsc::channel::<uuid::Uuid>(10);

    // Обработчик для Content фрейма, который нам будет присылать сервер
    tokio::spawn(async move {
        while let Some(Ok(frame)) = client_reader.read().await {
            match frame {
                protocol::PupaFrame::Content { msg_id, body } => {
                    flash_sender.send( msg_id ).await.unwrap()
                }
                protocol::PupaFrame::Win { msg_id, body } => {
                    println!("Win | msg_id: {}, body: {:?}", msg_id, body);
                }
                _ => { /* Сервер не будет нам писать ничего кроме Content и Win, просто игнорируем */ }
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
                println!("Writing regular content");
                client_writer.write_content(uuid::Uuid::new_v4(), client_writer.generate_random_text()).await?;
            },
        }
    }

    Ok(())
}
