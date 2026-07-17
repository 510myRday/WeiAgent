export type AbortSignalHandle = {
  signal: AbortSignal;
  dispose: () => void;
};

export function createAbortSignal(
  timeoutMs: number,
  parentSignal?: AbortSignal,
): AbortSignalHandle {
  const controller = new AbortController();
  const abortFromParent = () => controller.abort(parentSignal?.reason);

  if (parentSignal?.aborted) {
    abortFromParent();
  } else {
    parentSignal?.addEventListener('abort', abortFromParent, { once: true });
  }

  const timeoutId = globalThis.setTimeout(() => {
    controller.abort(new DOMException('The operation timed out', 'TimeoutError'));
  }, timeoutMs);

  return {
    signal: controller.signal,
    dispose() {
      globalThis.clearTimeout(timeoutId);
      parentSignal?.removeEventListener('abort', abortFromParent);
    },
  };
}
