use crossbeam::channel;
use downcast::{downcast, Any};

use crate::prelude::AccessResources;
use crate::prelude::{Component, Entity, Query, World};
use crate::resource::{ReadResource, Resource, ResourceLookupError, WriteResource};
use crate::world::{dispatch_inner, LazyUpdate};
use crate::{
    builder::LazyEntityBuilder,
    prelude::{AccessDispatcher, AccessEntityStats, AccessQuery},
};

/// Data that is threaded through components.
///
/// When a message is dispatched to an entity, it goes through its components. A component with a handler for this type
/// registered with [`World::register_component`] gets its listener called, and then the updated event gets passed to the next
/// component ... and so on. Then, it's returned to the dispatcher.
pub trait Message: Any {}
downcast!(dyn Message);

/// A message handler that only needs immutable access to the component.
pub type MsgHandlerRead<C, E> =
    fn(this: &C, event: E, owner: Entity, access: &ListenerWorldAccess) -> E;
/// A message handler that needs mutable access to the component.
pub type MsgHandlerWrite<C, E> =
    fn(this: &mut C, event: E, owner: Entity, access: &ListenerWorldAccess) -> E;

pub(crate) enum MsgHandlerInner {
    Read(
        Box<
            dyn Send
                + Sync
                + Fn(
                    &dyn Component,
                    Box<dyn Message>,
                    Entity,
                    &ListenerWorldAccess,
                ) -> Box<dyn Message>,
        >,
    ),
    Write(
        Box<
            dyn Send
                + Sync
                + Fn(
                    &mut dyn Component,
                    Box<dyn Message>,
                    Entity,
                    &ListenerWorldAccess,
                ) -> Box<dyn Message>,
        >,
    ),
}

/// Way to access a world from a message listener.
///
/// Some of the changes here won't actually apply until `World::finalize` is called.
pub struct ListenerWorldAccess<'w> {
    lazy_updates: channel::Sender<LazyUpdate>,
    queued_message_tx: channel::Sender<(Box<dyn Message>, Entity)>,
    queued_message_rx: channel::Receiver<(Box<dyn Message>, Entity)>,

    pub(crate) world: &'w World,
}

impl<'w> ListenerWorldAccess<'w> {
    pub(crate) fn new(world: &'w World) -> Self {
        let (tx, rx) = channel::unbounded();
        Self {
            lazy_updates: world.lazy_sender.clone(),
            queued_message_tx: tx,
            queued_message_rx: rx,
            world,
        }
    }

    /// Queue dispatching a message to the given entity. That entity will get the message sent to it once the current
    /// entity is through threading the current message through its components.
    ///
    /// Because handling of the new message is delayed, you cannot get the updated value of the message.
    ///
    /// This is handy for dispatching messages which would otherwise mutate components currently locked.
    pub fn queue_dispatch<M: Message>(&self, target: Entity, msg: M) {
        self.queued_message_tx
            .send((Box::new(msg), target))
            .unwrap();
    }

    /// Set up an entity to be spawned once [`World::finalize`] is called.
    pub fn lazy_spawn<'a>(&'a self) -> LazyEntityBuilder<'a, 'w> {
        let entity = self.world.entities.spawn_unfinished();
        LazyEntityBuilder::new(self, entity)
    }

    /// Queue an entity to be despawned when [`World::finalize`] is called.
    pub fn lazy_despawn(&self, entity: Entity) {
        self.queue_update(LazyUpdate::DespawnEntity(entity));
    }

    pub(crate) fn queue_update(&self, update: LazyUpdate) {
        self.lazy_updates.send(update).unwrap();
    }

    pub(crate) fn queued_message_rx(&self) -> &channel::Receiver<(Box<dyn Message>, Entity)> {
        &self.queued_message_rx
    }
}

impl<'w> AccessDispatcher for ListenerWorldAccess<'w> {
    fn dispatch<M: Message>(&self, target: Entity, msg: M) -> M {
        dispatch_inner(self, target, msg)
    }
}

impl<'w> AccessEntityStats for ListenerWorldAccess<'w> {
    fn len(&self) -> usize {
        self.world.len()
    }

    fn is_alive(&self, entity: Entity) -> bool {
        self.world.is_alive(entity)
    }

    fn len_of(&self, entity: Entity) -> usize {
        self.world.len_of(entity)
    }

    fn iter(&self) -> crate::entities::EntityIter<'_> {
        self.world.iter()
    }
}

impl<'w> AccessQuery for ListenerWorldAccess<'w> {
    fn query<'c, Q: Query<'c>>(&'c self, interrogatee: Entity) -> Option<Q::Response> {
        self.world.query::<Q>(interrogatee)
    }
}

impl<'w> AccessResources for ListenerWorldAccess<'w> {
    fn read_resource<R: Resource>(&self) -> Result<ReadResource<'_, R>, ResourceLookupError> {
        self.world.read_resource()
    }

    fn write_resource<R: Resource>(&self) -> Result<WriteResource<'_, R>, ResourceLookupError> {
        self.world.write_resource()
    }

    fn contains_resource<R: Resource>(&self) -> bool {
        self.world.contains_resource::<R>()
    }
}
