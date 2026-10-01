// Pages and metadata exactly as the web UI's message-page-operations.ts writes
// them (saveMessagePage / saveMetadata, with encodeStoredReactions), for an
// account (own cid 1001) talking to a peer (2002). These are what the agent's
// conversation store must read: history written before the agent owned it.
//
//   node generate.mjs <this directory>     (cbor-x must be resolvable)
import { encode as cborEncode } from 'cbor-x';
import { writeFileSync } from 'node:fs';

const encodeStoredReactions = (list) => (list && list.length > 0 ? Array.from(cborEncode(list)) : undefined);

const page = {
  peerCid: 2002n,
  pageNumber: 0,
  messages: [
    { id: 'm1', content: 'hi', senderCid: 1001n, recipientCid: 2002n, timestamp: 1790000000000, index: 1,
      status: 'read', replyTo: undefined, mentions: undefined, attachments: undefined, message_type: 'text',
      document_id: undefined, document_title: undefined },
    { id: 'm2', content: 'hello 👋', senderCid: 2002n, recipientCid: 1001n, timestamp: 1790000001000.25, index: 2,
      status: 'delivered', replyTo: 'm1', mentions: ['alice'], message_type: 'markdown', edited_at: 1790000002000,
      reactions: [
        { emoji: '👍', reactorCid: 1001n, at: 1790000003000, active: true },
        { emoji: '❤️', reactorCid: 2002n, at: 1790000003500, active: false },
      ] },
    { id: 'm3', content: 'report.pdf', senderCid: 1001n, recipientCid: 2002n, timestamp: 1790000004000, index: 3,
      status: 'sent', message_type: 'file_transfer', transfer_id: 't1', file_name: 'report.pdf', file_size: 123456,
      file_type: 'application/pdf', transfer_mode: 'async', transfer_state: 'staged', transfer_progress: 0.5,
      virtual_path: '/inbox/report.pdf', attachments: [{ file_id: 'f1', file_name: 'report.pdf', file_size: 123456, file_type: 'application/pdf' }] },
    { id: 'm4', content: 'oops', senderCid: 1001n, recipientCid: 2002n, timestamp: 1790000005000, index: 4,
      status: 'failed', error: 'Could not be saved', message_type: 'text', document_id: 'doc', document_title: 'Doc' },
  ],
  pageTimestamps: { minTimestamp: 1790000000000, maxTimestamp: 1790000005000 },
};
const metadata = {
  peerCid: 2002n, ownerCid: 1001n, peerUsername: 'bob', totalMessageCount: 4,
  oldestMessageTimestamp: 1790000000000, newestMessageTimestamp: 1790000005000, latestPage: 0,
  messagesPerPage: 50, unreadCount: 1, lastMessageIndex: 4, lastUpdated: 1790000006000,
};
// Written before ownership stamps: no ownerCid at all.
const legacyMetadata = { ...metadata, ownerCid: undefined };

const savePage = (p) => JSON.stringify({ ...p, peerCid: p.peerCid.toString(), messages: p.messages.map((m) => ({
  ...m, senderCid: m.senderCid.toString(), recipientCid: m.recipientCid.toString(), reactions: encodeStoredReactions(m.reactions),
})) });
const saveMetadata = (m) => JSON.stringify({ ...m, peerCid: m.peerCid.toString(),
  ownerCid: m.ownerCid === undefined ? undefined : m.ownerCid.toString() });

const dir = process.argv[2];
writeFileSync(`${dir}/page-0.json`, savePage(page));
writeFileSync(`${dir}/metadata.json`, saveMetadata(metadata));
writeFileSync(`${dir}/metadata-unattributed.json`, saveMetadata(legacyMetadata));
console.log('wrote 3 fixtures');
