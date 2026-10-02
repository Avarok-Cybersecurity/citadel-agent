//! Where a message ILM delivered goes.

use super::HostIo;
use async_trait::async_trait;
use citadel_internal_service_connector::messenger::WrappedMessage;
use citadel_internal_service_types::{InternalServicePayload, InternalServiceResponse};
use intersession_layer_messaging::local_delivery::LocalDelivery;
use intersession_layer_messaging::{DeliveryError, MessageMetadata};
use std::sync::Arc;

/// Delivery to the account, through [`HostIo::deliver`].
///
/// Refused when nobody takes it, so ILM neither acknowledges nor clears the
/// message and tries again: the sender keeps it until it is really delivered.
pub(crate) struct AgentDelivery {
    pub(crate) cid: u64,
    pub(crate) io: Arc<dyn HostIo>,
}

#[async_trait]
impl LocalDelivery<WrappedMessage> for AgentDelivery {
    async fn deliver(&self, message: WrappedMessage) -> Result<(), DeliveryError> {
        let InternalServicePayload::Response(InternalServiceResponse::MessageNotification(
            notification,
        )) = message.contents().clone()
        else {
            return Err(DeliveryError::BadInput);
        };
        if self.io.deliver(self.cid, notification).await {
            Ok(())
        } else {
            Err(DeliveryError::NoReceiver)
        }
    }
}
