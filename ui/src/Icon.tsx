// Icônes SVG inline 24px, stroke 2px (charte graphique, style Material Symbols).

import type { ReactNode } from "react";
export function Icon({ name, size = 24, filled = false, className = "" }: { name: string; size?: number; filled?: boolean; className?: string }) {
  const P: Record<string, ReactNode> = {
    home: (<><path d="M3 11l9-8 9 8" /><path d="M5 9.5V21h14V9.5" /></>),
    note: (<><path d="M9 18V5l12-2v13" /><circle cx="6" cy="18" r="3" /><circle cx="18" cy="16" r="3" /></>),
    folder: <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7z" />,
    queue: <path d="M4 6h16M4 12h16M4 18h10" />,
    playlist: (<><path d="M3 6h12M3 10h12M3 14h7" /><circle cx="17" cy="17" r="3" /><path d="M20 17V7l-3-1" /></>),
    gear: (<><circle cx="12" cy="12" r="3.2" /><path d="M12 2.5v3M12 18.5v3M2.5 12h3M18.5 12h3M5.3 5.3l2.1 2.1M16.6 16.6l2.1 2.1M18.7 5.3l-2.1 2.1M7.4 16.6l-2.1 2.1" /></>),
    search: (<><circle cx="11" cy="11" r="7" /><path d="m20.5 20.5-4-4" /></>),
    usb: (<><path d="M9 3v4M15 3v4" /><path d="M7 7h10v4a5 5 0 0 1-10 0V7z" /><path d="M12 16v5" /><circle cx="12" cy="13.5" r="0.5" fill="currentColor" /></>),
    server: (<><rect x="3" y="4" width="18" height="7" rx="2" /><rect x="3" y="13" width="18" height="7" rx="2" /><path d="M7 7.5h.01M7 16.5h.01" strokeWidth="2.6" /></>),
    radio: (<><rect x="3" y="9" width="18" height="11" rx="2" /><path d="M7 9l11-5" /><circle cx="8" cy="14.5" r="1.8" /><path d="M13.5 12.5h4M13.5 16h4" /></>),
    link: (<><path d="M10 14a5 5 0 0 0 7.1 0l2.9-2.9a5 5 0 0 0-7.1-7.1L11.5 5.4" /><path d="M14 10a5 5 0 0 0-7.1 0L4 12.9a5 5 0 0 0 7.1 7.1l1.4-1.4" /></>),
    play: <path d="M8 5.5v13l11-6.5z" fill="currentColor" stroke="none" />,
    pause: <path d="M7 5h3.5v14H7zM13.5 5H17v14h-3.5z" fill="currentColor" stroke="none" />,
    next: (<><path d="M6 5.5v13l8.5-6.5z" fill="currentColor" stroke="none" /><rect x="15.5" y="5" width="2.6" height="14" rx="1" fill="currentColor" stroke="none" /></>),
    prev: (<><path d="M18 5.5v13L9.5 12z" fill="currentColor" stroke="none" /><rect x="5.9" y="5" width="2.6" height="14" rx="1" fill="currentColor" stroke="none" /></>),
    stop: <rect x="6" y="6" width="12" height="12" rx="1.5" fill="currentColor" stroke="none" />,
    shuffle: (<><path d="M16 4h4v4" /><path d="M4 20 20 4" /><path d="M20 16v4h-4" /><path d="m14.5 14.5 5.5 5.5" /><path d="M4 4l5.5 5.5" /></>),
    repeat: (<><path d="m17 2.5 4 4-4 4" /><path d="M3 11.5v-1a4 4 0 0 1 4-4h14" /><path d="m7 21.5-4-4 4-4" /><path d="M21 12.5v1a4 4 0 0 1-4 4H3" /></>),
    heart: <path d="M12 20.5C7 16.5 3.5 13.3 3.5 9.6 3.5 7 5.5 5 8 5c1.6 0 3.1.8 4 2.1C12.9 5.8 14.4 5 16 5c2.5 0 4.5 2 4.5 4.6 0 3.7-3.5 6.9-8.5 10.9z" />,
    dots: (<><circle cx="5" cy="12" r="1.6" fill="currentColor" stroke="none" /><circle cx="12" cy="12" r="1.6" fill="currentColor" stroke="none" /><circle cx="19" cy="12" r="1.6" fill="currentColor" stroke="none" /></>),
    download: (<><path d="M12 3v11" /><path d="m7 10 5 5 5-5" /><path d="M4 20h16" /></>),
    copy: (<><rect x="9" y="9" width="12" height="12" rx="2" /><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" /></>),
    trash: (<><path d="M4 7h16" /><path d="M9 7V4h6v3" /><path d="M6 7l1 13a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-13" /></>),
    edit: <path d="M17 3.5a2.6 2.6 0 1 1 3.7 3.7L7.5 20.4 2.5 21.5l1.1-5z" />,
    close: <path d="M6 6l12 12M18 6 6 18" />,
    plus: <path d="M12 5v14M5 12h14" />,
    grid: (<><rect x="4" y="4" width="7" height="7" rx="1.5" /><rect x="13" y="4" width="7" height="7" rx="1.5" /><rect x="4" y="13" width="7" height="7" rx="1.5" /><rect x="13" y="13" width="7" height="7" rx="1.5" /></>),
    list: <path d="M4 6h16M4 12h16M4 18h16" />,
    back: (<><path d="M19 12H5" /><path d="m11 18-6-6 6-6" /></>),
    chevron: <path d="m6 9 6 6 6-6" />,
    eject: (<><path d="M12 15V4" /><path d="m7 9 5-5 5 5" /><path d="M4 19h16" /></>),
  };
  return (
    <svg
      width={size} height={size} viewBox="0 0 24 24" fill="none"
      stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round"
      className={`ico-svg ${className}`} aria-hidden="true"
      style={filled ? { fill: "currentColor" } : undefined}
    >
      {P[name] ?? P.note}
    </svg>
  );
}
