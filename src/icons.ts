// Iconos lineales finos (16×16, trazo 1.25, extremos rectos). Heredan `currentColor`.

const paths = {
  mouse: '<rect x="4.5" y="1.5" width="7" height="13" rx="3.5"/><path d="M8 1.5v4"/>',
  keyboard:
    '<rect x="1.5" y="4.5" width="13" height="7"/><path d="M4 7h1M7 7h1M10 7h1M4.5 9.5h7"/>',
  headset:
    '<path d="M3 10V8a5 5 0 0 1 10 0v2"/><rect x="2" y="9.5" width="2.5" height="4"/><rect x="11.5" y="9.5" width="2.5" height="4"/>',
  gamepad:
    '<rect x="1.5" y="5" width="13" height="7"/><path d="M4 8.5h3M5.5 7v3M10.5 7.5v.5M12 9.5v.5"/>',
  other:
    '<rect x="4.5" y="4.5" width="7" height="7"/><path d="M6.5 2v2.5M9.5 2v2.5M6.5 11.5V14M9.5 11.5V14M2 6.5h2.5M2 9.5h2.5M11.5 6.5H14M11.5 9.5H14"/>',
  receiver: '<rect x="5.5" y="1.5" width="5" height="4"/><rect x="4.5" y="5.5" width="7" height="9"/><path d="M7 3.5h2"/>',
  wired: '<path d="M5.5 1.5v3M10.5 1.5v3"/><rect x="3.5" y="4.5" width="9" height="5"/><path d="M8 9.5v5"/>',
  bluetooth: '<path d="M4.5 5l7 6L8 14V2l3.5 3-7 6"/>',
  virtual: '<rect x="1.5" y="1.5" width="13" height="13" stroke-dasharray="2 2"/><path d="M5 8h6"/>',
  bolt: '<path d="M9 1.5L4 9h4l-1 5.5L12 7H8z"/>',
  battery: '<rect x="1.5" y="4.5" width="12" height="7"/><path d="M13.5 6.5h1v3h-1M4 7v2M6.5 7v2"/>',
  clock: '<circle cx="8" cy="8" r="6.5"/><path d="M8 4.5V8l2.5 1.5"/>',
  scan: '<path d="M13.5 8A5.5 5.5 0 1 1 11.6 3.9"/><path d="M12 1.5v3h-3"/>',
  sliders:
    '<path d="M2 4.5h12M2 11.5h12"/><rect x="9" y="3" width="2.5" height="3" fill="currentColor"/><rect x="4.5" y="10" width="2.5" height="3" fill="currentColor"/>',
  pulse: '<path d="M1.5 8h3l1.5-4 3 8 1.5-4h4"/>',
  root: '<rect x="1.5" y="1.5" width="13" height="13"/><path d="M1.5 5.5h13M5.5 5.5v9"/>',
  min: '<path d="M3.5 8h9"/>',
  close: '<path d="M3.5 3.5l9 9M12.5 3.5l-9 9"/>',
  warn: '<path d="M8 2l6.5 11.5h-13z"/><path d="M8 6.5v3.5M8 11.5v.5"/>',
  edit: '<path d="M10.5 2.5l3 3L6 13H3v-3z"/><path d="M9 4l3 3"/>',
} as const;

export type IconName = keyof typeof paths;

export function icon(name: IconName, cls = ""): string {
  return `<svg class="ico ${cls}" viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="square" aria-hidden="true">${paths[name]}</svg>`;
}
