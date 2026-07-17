import { readChatDraft, writeChatDraft } from './chatDrafts';

test('chat drafts are isolated by conversation key', () => {
  writeChatDraft('agent-a:conversation-a', 'first');
  writeChatDraft('agent-a:conversation-b', 'second');

  expect(readChatDraft('agent-a:conversation-a')).toBe('first');
  expect(readChatDraft('agent-a:conversation-b')).toBe('second');

  writeChatDraft('agent-a:conversation-a', '');
  expect(readChatDraft('agent-a:conversation-a')).toBe('');
});
