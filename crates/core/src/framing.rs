//! Length-prefixed `postcard` framing for the control channel.
//!
//! Each frame is a 4-byte big-endian length followed by that many bytes of a
//! `postcard`-serialized value. A configurable maximum guards against a peer
//! announcing an absurd length.

use crate::error::{Error, Result};
use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Hard cap on a single control frame (8 MiB). Control messages are tiny; this
/// only ever matters as a denial-of-service guard.
pub const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;

/// Serialize `msg` and write it as one length-prefixed frame.
pub async fn write_msg<W, T>(w: &mut W, msg: &T) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let body = postcard::to_stdvec(msg)?;
    if body.len() > MAX_FRAME_LEN {
        return Err(Error::FrameTooLarge(body.len(), MAX_FRAME_LEN));
    }
    let len = (body.len() as u32).to_be_bytes();
    w.write_all(&len).await?;
    w.write_all(&body).await?;
    w.flush().await?;
    Ok(())
}

/// Read one length-prefixed frame and deserialize it.
///
/// Returns `Error::Io` with `UnexpectedEof` if the peer closes cleanly between
/// frames — callers treat that as "connection ended".
pub async fn read_msg<R, T>(r: &mut R) -> Result<T>
where
    R: AsyncReadExt + Unpin,
    T: DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_LEN {
        return Err(Error::FrameTooLarge(len, MAX_FRAME_LEN));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok(postcard::from_bytes(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ControlMsg;

    #[tokio::test]
    async fn roundtrip_single_frame() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let msg = ControlMsg::Ping { nonce: 42 };
        let expect = msg.clone();
        tokio::spawn(async move {
            write_msg(&mut a, &msg).await.unwrap();
        });
        let got: ControlMsg = read_msg(&mut b).await.unwrap();
        assert_eq!(got, expect);
    }

    #[tokio::test]
    async fn roundtrip_many_frames_in_order() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let msgs: Vec<ControlMsg> = (0..50).map(|n| ControlMsg::Ping { nonce: n }).collect();
        let to_send = msgs.clone();
        tokio::spawn(async move {
            for m in &to_send {
                write_msg(&mut a, m).await.unwrap();
            }
        });
        for expect in &msgs {
            let got: ControlMsg = read_msg(&mut b).await.unwrap();
            assert_eq!(&got, expect);
        }
    }

    #[tokio::test]
    async fn oversized_len_is_rejected() {
        // Hand-craft a frame header announcing more than MAX_FRAME_LEN.
        let (mut a, mut b) = tokio::io::duplex(64);
        tokio::spawn(async move {
            let bogus = (MAX_FRAME_LEN as u32 + 1).to_be_bytes();
            a.write_all(&bogus).await.unwrap();
            a.flush().await.unwrap();
        });
        let res: Result<ControlMsg> = read_msg(&mut b).await;
        assert!(matches!(res, Err(Error::FrameTooLarge(_, _))));
    }
}
