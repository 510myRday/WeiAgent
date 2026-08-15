import type { UiMessageItem } from '../../types';

/** Changes when a task-planning tool call reaches a new execution result. */
export function getTaskPlanSyncKey(messages: UiMessageItem[]): string {
  return messages
    .flatMap((message) => message.blocks ?? [])
    .filter((block): block is Extract<NonNullable<UiMessageItem['blocks']>[number], { type: 'tool' }> => (
      block.type === 'tool' && block.tool.name === 'task_planning'
    ))
    .map((block) => `${block.tool.call_id}:${block.tool.status}:${JSON.stringify(block.tool.result ?? null)}`)
    .join('|');
}
