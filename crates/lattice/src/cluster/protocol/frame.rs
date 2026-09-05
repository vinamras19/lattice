use super::message::Message;
use crate::error::LatticeError;
use bytes::{Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};

pub struct MessageCodec {
    inner: LengthDelimitedCodec,
}

impl MessageCodec {
    pub fn new() -> Self {
        Self { inner: LengthDelimitedCodec::new() }
    }
}

impl Default for MessageCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder for MessageCodec {
    type Item = Message;
    type Error = LatticeError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Message>, LatticeError> {
        match self.inner.decode(src)? {
            Some(frame) => Ok(Some(Message::deserialize(&frame)?)),
            None => Ok(None),
        }
    }
}

impl Encoder<Message> for MessageCodec {
    type Error = LatticeError;

    fn encode(&mut self, item: Message, dst: &mut BytesMut) -> Result<(), LatticeError> {
        self.inner.encode(Bytes::from(item.serialize()), dst)?;
        Ok(())
    }
}