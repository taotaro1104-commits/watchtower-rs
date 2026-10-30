// 履歴が増えたらページを更新する (SSE)。入力中は更新しない。
(() => {
  if (!('EventSource' in window)) return;
  const source = new EventSource('/events/stream');
  let timer = null;
  source.addEventListener('change', () => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      const active = document.activeElement;
      if (active && ['INPUT', 'TEXTAREA', 'SELECT'].includes(active.tagName)) return;
      location.reload();
    }, 500);
  });
})();
