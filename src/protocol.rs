use std::io;

use serde::{Deserialize, Serialize};
use tokio_util::codec::{Decoder, Encoder};

// Фрейм нашего протокола. Несмотря на то, что мы используем
// TCP, где данные передаются просто, как стрим байтов, мы
// можем выделить логические блоки, которые называются фреймами.
// В этом файле мы реализуем протокол и определяем фреймы, которыми будут обмениваться
// клиент и сервер.
#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
pub enum PupaFrame {
    // Фрейм для авторизации
    Authorize {
        signature: uuid::Uuid,
    },
    NonAuthorized,
    Content {
        msg_id: uuid::Uuid,
        body: Vec<u8>,
    },
    Flash {
        msg_id: uuid::Uuid,
    },
    Win {
        msg_id: uuid::Uuid,
        body: Vec<u8>,
    },
    ShowWinners,
    WinnerRecord {
        signature: uuid::Uuid,
        online: bool,
        wins: u32,
        messages_received: u32,
        messages_sent: u32,
    },
    ShowWinnersLog,
    WinLogRecord {
        signature: uuid::Uuid,
        timestamp: u128,
        msg_id: uuid::Uuid,
    },
}

// Кодек позволяет нам превратить наш фрейм в байты и обратно.
// Мы для передачи данных будем использовать бинкод
// (можно и другое что-то, но бинкод для старта вполне подойдет)
#[derive(Clone)]
pub struct PupaCodec {
    already_waited: bool,
}

impl PupaCodec {
    pub fn new() -> PupaCodec {
        PupaCodec {
            already_waited: false,
        }
    }
}

impl Encoder<PupaFrame> for PupaCodec {
    type Error = io::Error;

    fn encode(&mut self, item: PupaFrame, buffer: &mut bytes::BytesMut) -> Result<(), io::Error> {
        let encoded: Vec<u8> =
            bincode::serialize(&item).expect("unvalidated data passed to encoder");
        buffer.extend(encoded);
        Ok(())
    }
}

impl Decoder for PupaCodec {
    type Item = PupaFrame;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut bytes::BytesMut) -> Result<Option<PupaFrame>, io::Error> {
        if !buf.is_empty() {
            match bincode::deserialize::<PupaFrame>(&buf[..]) {
                Ok(decoded) => match bincode::serialized_size(&decoded) {
                    Ok(already_consumed) => {
                        let _consumed_frame = buf.split_to(already_consumed as usize);
                        // Если мы до этого включали коррекцию, то выключим ее
                        self.already_waited = false;
                        Ok(Some(decoded))
                    }
                    Err(_) => Err(io::Error::new(
                        io::ErrorKind::Other,
                        "Failed to calculate serialized size",
                    )),
                },
                Err(_err) => {
                    // Протокол продуман не до конца, поэтому осталась одна проблема,
                    // если мы читаем не до конца заполненный буфер. Перед тем как
                    // считать фрейм битым, попробуем подождать чуть-чуть и прочитать
                    // его на следующей итерации.
                    // Такая коррекция фрейма допустима, но в слабозагруженной системе может привести
                    // к неожиданной задержке в обработке фреймов. Но тут позволим себе такое.
                    if self.already_waited {
                        self.already_waited = false;
                        buf.clear();
                        Err(io::Error::new(
                            io::ErrorKind::Other,
                            "Failed to decode Frame, cleaning buffer",
                        ))
                    } else {
                        self.already_waited = true;
                        Ok(None)
                    }
                }
            }
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_encoder_decoder() {
        let frame = PupaFrame::Content {
            msg_id: uuid::Uuid::new_v4(),
            body: vec![1, 2, 3, 4, 5],
        };

        let mut buffer = bytes::BytesMut::new();
        PupaCodec::new().encode(frame.clone(), &mut buffer).unwrap();

        let decoded = PupaCodec::new().decode(&mut buffer).unwrap().unwrap();
        assert_eq!(frame, decoded)
    }

    #[test]
    fn test_frame_encoder_decoder_on_multiplexed_stream() {
        let frame1 = PupaFrame::Content {
            msg_id: uuid::Uuid::new_v4(),
            body: vec![1, 2, 3, 4, 5],
        };

        let frame2 = PupaFrame::Content {
            msg_id: uuid::Uuid::new_v4(),
            body: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };

        let mut buffer = bytes::BytesMut::new();

        PupaCodec::new()
            .encode(frame1.clone(), &mut buffer)
            .unwrap();
        PupaCodec::new()
            .encode(frame2.clone(), &mut buffer)
            .unwrap();

        let decoded1 = PupaCodec::new().decode(&mut buffer).unwrap().unwrap();
        let decoded2 = PupaCodec::new().decode(&mut buffer).unwrap().unwrap();

        assert_eq!(frame1, decoded1);
        assert_eq!(frame2, decoded2);
        assert_eq!(buffer.len(), 0);
    }
}
