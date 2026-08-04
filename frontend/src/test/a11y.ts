import axe from 'axe-core';

export function runA11yCheck(container: Element) {
  return axe.run(container, {
    resultTypes: ['violations'],
  });
}
