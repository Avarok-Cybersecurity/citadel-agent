// Captures the bytes the web UI's cbor-x (1.6.0, default options, as
// `serializeP2PCommand` and `encodeStoredReactions` use it) produces for each
// P2P command shape the agent reads or writes, and for a stored reaction list.
//
// The object literals follow the UI's constructors key for key
// (types/p2p-commands.ts createMessagingLayerCommand / createMessageAckCommand,
// types/messaging-layer.ts, types/message-reaction-layer.ts,
// lib/reactions/stored-reactions.ts). The Rust codec must decode every file and
// re-encode it to the identical bytes (kernel/conversations/cbor tests).
//
// Run with cbor-x resolvable, naming this directory:
//   node generate.mjs <this directory>
import { encode } from 'cbor-x';
import { writeFileSync } from 'node:fs';

const fixtures = {
  'message': {
    type: 'MessagingLayerCommand',
    payload: {
      layer: { type: 'Message', contents: 'hello', timestamp: 1790000000123 },
      sender_cid: 1001n, recipient_cid: 2002n,
      message_id: '0b5f2b0e-3c8e-4b7a-9f00-2a4c7e1d9a11', index: 7,
      reply_to: undefined, mentions: undefined, attachments: undefined,
      message_type: 'text', document_id: undefined, document_title: undefined,
    },
  },
  'message-full': {
    type: 'MessagingLayerCommand',
    payload: {
      layer: { type: 'Message', contents: 'héllo 👋 — ünïcode', timestamp: 1790000000123.5 },
      sender_cid: 18446744073709551000n, recipient_cid: 3n,
      message_id: 'id-full', index: 5000000000,
      reply_to: 'id-parent', mentions: ['alice', 'bob'],
      attachments: [{ file_id: 'f1', file_name: 'a.txt', file_size: 12, file_type: 'text/plain', thumbnail: undefined }],
      message_type: 'live_document', document_id: 'doc-1', document_title: 'Plan',
    },
  },
  'message-long': {
    type: 'MessagingLayerCommand',
    payload: {
      layer: { type: 'Message', contents: 'x'.repeat(23) + 'y'.repeat(300) + '€'.repeat(30000), timestamp: 4294967295 },
      sender_cid: 0n, recipient_cid: 4294967296n,
      message_id: 'm'.repeat(24), index: 4294967296,
      reply_to: undefined, mentions: [], attachments: undefined,
      message_type: 'markdown', document_id: undefined, document_title: undefined,
    },
  },
  'ack-delivered': {
    type: 'MessageAck',
    payload: { ack_type: 'delivered', message_id: 'id-1', timestamp: 1790000000999, error: undefined },
  },
  'ack-failed': {
    type: 'MessageAck',
    payload: { ack_type: 'failed', message_id: 'id-2', timestamp: 12, error: 'nope' },
  },
  'edit': {
    type: 'MessagingLayerCommand',
    payload: {
      layer: { type: 'MessageEdit', message_id: 'id-1', contents: 'hello, edited', edited_at: 1790000001000 },
      sender_cid: 1001n, recipient_cid: 2002n, message_id: 'cmd-edit', index: 7,
      reply_to: undefined, mentions: undefined, attachments: undefined,
      message_type: 'text', document_id: undefined, document_title: undefined,
    },
  },
  'delete': {
    type: 'MessagingLayerCommand',
    payload: {
      layer: { type: 'MessageDelete', message_id: 'id-1', deleted_at: 1790000002000 },
      sender_cid: 1001n, recipient_cid: 2002n, message_id: 'cmd-delete', index: 7,
      reply_to: undefined, mentions: undefined, attachments: undefined,
      message_type: 'text', document_id: undefined, document_title: undefined,
    },
  },
  'reaction': {
    type: 'MessagingLayerCommand',
    payload: {
      layer: { type: 'MessageReaction', message_id: 'id-1', emoji: '👍', active: true, reacted_at: 1790000003000 },
      sender_cid: 1001n, recipient_cid: 2002n, message_id: 'cmd-react', index: 7,
      reply_to: undefined, mentions: undefined, attachments: undefined,
      message_type: 'text', document_id: undefined, document_title: undefined,
    },
  },
  'screenshot': {
    type: 'MessagingLayerCommand',
    payload: {
      layer: { type: 'ScreenshotNotice', taken_at: 1790000004000 },
      sender_cid: 1001n, recipient_cid: 2002n, message_id: 'cmd-shot', index: 7,
      reply_to: undefined, mentions: undefined, attachments: undefined,
      message_type: 'text', document_id: undefined, document_title: undefined,
    },
  },
  'typing': {
    type: 'MessagingLayerCommand',
    payload: {
      layer: { type: 'Typing' },
      sender_cid: 1001n, recipient_cid: 2002n, message_id: 'cmd-typing', index: 7,
      reply_to: undefined, mentions: undefined, attachments: undefined,
      message_type: 'text', document_id: undefined, document_title: undefined,
    },
  },
  'call-signal': {
    type: 'CallSignal',
    payload: { kind: 'CallEnd', call_id: 'call-1', reason: 'hangup' },
  },
  // A ring, as websocket-call-transport.ts sends it: what raises a native call notice.
  'call-invite': {
    type: 'CallSignal',
    payload: {
      kind: 'CallInvite', call_id: 'call-2',
      media: { audio: true, video: true, screen: false },
      codecs: { audio: ['opus'], video: [{ codec: 'vp8', hardware: false, maxHeight: 720 }] },
      media_wire_version: 1, video_send_codec: 'vp8',
    },
  },
  'negative-and-small': {
    type: 'MessageAck',
    payload: { ack_type: 'read', message_id: '', timestamp: -5, error: undefined },
  },
  // Encoder rules on their own: the Rust side BUILDS these and compares.
  'numbers': [0, 23, 24, 255, 256, 65535, 65536, 4294967295, 4294967296, -1, -24, -25, -256, -257,
    -2147483648, -2147483649, 1.5, 0.1, 1790000000123, -0, 1e300],
  'strings': ['', 'a'.repeat(23), 'a'.repeat(24), 'a'.repeat(255), 'a'.repeat(256), 'a'.repeat(65536), 'é'.repeat(100)],
  'bigints': [0n, 5n, 18446744073709551615n],
  'reactions': [
    { emoji: '👍', reactorCid: 1001n, at: 1790000003000, active: true },
    { emoji: '❤️', reactorCid: 2002n, at: 1790000003500, active: false },
    { emoji: '🎉', reactorCid: 7n, at: 3, active: true },
  ],
};

for (const [name, value] of Object.entries(fixtures)) {
  writeFileSync(`${process.argv[2]}/${name}.cbor`, encode(value));
}
console.log(`wrote ${Object.keys(fixtures).length} fixtures`);
