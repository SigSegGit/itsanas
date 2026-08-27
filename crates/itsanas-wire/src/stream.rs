//! A framed conversation over any byte stream.
//!
//! Generic over the stream on purpose. A plain `TcpStream` and a TLS session
//! are both `Read + Write`, so the protocol layers above never learn which one
//! they are running on — and adding encryption later becomes a change of type
//! rather than a change of every call site. That is worth more than it sounds:
//! a transport upgrade that touches the message-handling code is a transport
//! upgrade that can quietly change message handling.

use std::io::{Read, Write};

use serde::{Serialize, de::DeserializeOwned};

use crate::wire::{self, FrameReader, WireError};

/// Everything that can go wrong on a framed stream.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("framing: {0}")]
    Wire(#[from] WireError),

    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),

    #[error("the connection closed part-way through a message")]
    TruncatedClose,
}

/// One end of a framed conversation.
#[derive(Debug)]
pub struct Connection<S> {
    stream: S,
    reader: FrameReader,
}

impl<S: Read + Write> Connection<S> {
    #[must_use]
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            reader: FrameReader::new(),
        }
    }

    /// The underlying stream, for setting timeouts and the like.
    pub fn stream(&mut self) -> &mut S {
        &mut self.stream
    }

    /// Send one message.
    pub fn send<T: Serialize>(&mut self, message: &T) -> Result<(), StreamError> {
        let frame = wire::encode(message)?;
        self.stream.write_all(&frame)?;
        self.stream.flush()?;
        Ok(())
    }

    /// Read until one whole message has arrived.
    ///
    /// `Ok(None)` means the peer closed cleanly *between* messages. A close
    /// part-way through one is an error, because silently discarding a partial
    /// frame would let a peer truncate a response and have it treated as
    /// complete.
    pub fn receive<T: DeserializeOwned>(&mut self) -> Result<Option<T>, StreamError> {
        loop {
            if let Some(message) = self.reader.next_message()? {
                return Ok(Some(message));
            }

            let mut buffer = [0u8; 16 * 1024];
            match self.stream.read(&mut buffer)? {
                0 => {
                    return if self.reader.buffered() == 0 {
                        Ok(None)
                    } else {
                        Err(StreamError::TruncatedClose)
                    };
                }
                read => self.reader.push(&buffer[..read]),
            }
        }
    }

    /// Send a request and wait for its answer.
    pub fn exchange<Q: Serialize, A: DeserializeOwned>(
        &mut self,
        request: &Q,
    ) -> Result<A, StreamError> {
        self.send(request)?;
        self.receive()?.ok_or(StreamError::TruncatedClose)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Message {
        number: u64,
        text: String,
    }

    /// An in-memory duplex: what is written can be read back.
    #[derive(Default)]
    struct Loopback {
        buffer: Vec<u8>,
        at: usize,
    }

    impl Read for Loopback {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let available = self.buffer.len() - self.at;
            let take = available.min(out.len());
            out[..take].copy_from_slice(&self.buffer[self.at..self.at + take]);
            self.at += take;
            Ok(take)
        }
    }

    impl Write for Loopback {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buffer.extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn message(number: u64) -> Message {
        Message {
            number,
            text: format!("message {number}"),
        }
    }

    #[test]
    fn a_message_round_trips() {
        let mut connection = Connection::new(Loopback::default());
        connection.send(&message(1)).unwrap();
        assert_eq!(connection.receive::<Message>().unwrap(), Some(message(1)));
    }

    #[test]
    fn several_messages_come_back_in_order() {
        let mut connection = Connection::new(Loopback::default());
        for number in 0..5 {
            connection.send(&message(number)).unwrap();
        }
        for number in 0..5 {
            assert_eq!(
                connection.receive::<Message>().unwrap(),
                Some(message(number))
            );
        }
    }

    #[test]
    fn a_clean_close_between_messages_is_not_an_error() {
        // How a peer says goodbye. Treating it as a fault would fill logs with
        // errors for the most ordinary thing a connection does.
        let mut connection = Connection::new(Loopback::default());
        assert_eq!(connection.receive::<Message>().unwrap(), None);
    }

    #[test]
    fn a_close_part_way_through_a_message_is_an_error() {
        // Silently discarding a partial frame would let a peer truncate a
        // response and have it treated as complete.
        let frame = wire::encode(&message(1)).unwrap();

        let mut connection = Connection::new(Loopback {
            buffer: frame[..frame.len() - 1].to_vec(),
            at: 0,
        });

        assert!(matches!(
            connection.receive::<Message>(),
            Err(StreamError::TruncatedClose)
        ));
    }

    #[test]
    fn an_oversized_frame_is_refused_rather_than_buffered_towards() {
        let mut header = vec![crate::WIRE_VERSION];
        header.extend_from_slice(&u32::MAX.to_le_bytes());

        let mut connection = Connection::new(Loopback {
            buffer: header,
            at: 0,
        });

        assert!(matches!(
            connection.receive::<Message>(),
            Err(StreamError::Wire(_))
        ));
    }
}
