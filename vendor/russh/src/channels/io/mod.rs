mod rx;
use std::borrow::{Borrow, BorrowMut};

pub use rx::ChannelRx;

mod tx;
pub use tx::ChannelTx;

use crate::{Channel, ChannelId, ChannelMsg, ChannelReadHalf};

#[derive(Debug)]
pub struct ChannelCloseOnDrop<S: From<(ChannelId, ChannelMsg)> + Send + 'static>(pub Channel<S>);

impl<S: From<(ChannelId, ChannelMsg)> + Send + 'static> Borrow<ChannelReadHalf>
    for ChannelCloseOnDrop<S>
{
    fn borrow(&self) -> &ChannelReadHalf {
        &self.0.read_half
    }
}

impl<S: From<(ChannelId, ChannelMsg)> + Send + 'static> BorrowMut<ChannelReadHalf>
    for ChannelCloseOnDrop<S>
{
    fn borrow_mut(&mut self) -> &mut ChannelReadHalf {
        &mut self.0.read_half
    }
}

impl<S: From<(ChannelId, ChannelMsg)> + Send + 'static> Drop for ChannelCloseOnDrop<S> {
    fn drop(&mut self) {
        let id = self.0.write_half.id;
        let msg: S = (id, ChannelMsg::Close).into();

        // Olo: enqueue CLOSE synchronously on the driver's own channel (FIFO
        // `try_send` first, then unbounded `close_sender` if full) rather than
        // calling ambient `tokio::spawn`, which panics when dropped on a plain
        // OS thread.
        if let Err(tokio::sync::mpsc::error::TrySendError::Full(msg)) =
            self.0.write_half.sender.try_send(msg)
        {
            if let Some(close_sender) = &self.0.write_half.close_sender {
                let _ = close_sender.send(msg);
            } else {
                #[cfg(not(target_arch = "wasm32"))]
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    let sender = self.0.write_half.sender.clone();
                    handle.spawn(async move {
                        let _ = sender.send(msg).await;
                    });
                }
            }
        }
    }
}
