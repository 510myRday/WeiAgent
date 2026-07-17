import '@testing-library/jest-dom/vitest';

class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}

class IntersectionObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}

Object.defineProperty(globalThis, 'ResizeObserver', {
  value: ResizeObserverStub,
  configurable: true,
});
Object.defineProperty(globalThis, 'IntersectionObserver', {
  value: IntersectionObserverStub,
  configurable: true,
});
Object.defineProperty(HTMLElement.prototype, 'scrollTo', {
  value: () => {},
  configurable: true,
});
Object.defineProperty(HTMLElement.prototype, 'setPointerCapture', {
  value: () => {},
  configurable: true,
});
Object.defineProperty(HTMLElement.prototype, 'releasePointerCapture', {
  value: () => {},
  configurable: true,
});
Object.defineProperty(HTMLElement.prototype, 'hasPointerCapture', {
  value: () => false,
  configurable: true,
});
