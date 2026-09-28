//! Where an agent-hosted ILM delivers: the session's route, so every subscriber
//! of the session receives the message as a `MessageNotification`.
//!
//! "Nobody listening" is a failed delivery, not a dropped one. ILM then keeps
//! the message, sends no ACK and delivers it again next cycle, so a message
//! that arrives while no tab is open waits in the agent for the next one --
//! the guarantee the browser's ILM gave by simply not running.
use crate::kernel::session_route::SessionRoute;
use citadel_internal_service_connector::messenger::ilm::DeliveryError;
use citadel_internal_service_connector::messenger::DeliveryTarget;
use citadel_internal_service_types::InternalServiceResponse;

impl DeliveryTarget for SessionRoute {
    fn deliver_response(&self, response: InternalServiceResponse) -> Result<(), DeliveryError> {
        self.send(response)
            .map(|_delivered_to| ())
            .ok_or(DeliveryError::NoReceiver)
    }
}
