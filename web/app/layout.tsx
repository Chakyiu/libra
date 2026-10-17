import type { Metadata, Viewport } from "next";
import "@/styles/base.css";
import "@/styles/shell.css";
import "@/styles/reader.css";
import "@/styles/rail.css";
import "@/styles/composer.css";
import "@/styles/home.css";
import "@/styles/settings.css";
import "@/styles/pages.css";
import Pwa from "@/components/Pwa";
import Shell from "@/components/Shell";

// No metadata title: Next would re-apply it after hydration over the per-thread titles the client sets.
export const metadata: Metadata = {
  description: "A self-hosted answer engine that shows its work.",
  applicationName: "Libra",
  manifest: "/manifest.webmanifest",
  appleWebApp: {
    capable: true,
    title: "Libra",
    statusBarStyle: "default",
  },
  other: {
    "apple-mobile-web-app-capable": "yes",
  },
  icons: {
    icon: [
      { url: "/icon-192.png", sizes: "192x192", type: "image/png" },
      { url: "/icon-512.png", sizes: "512x512", type: "image/png" },
    ],
    apple: [{ url: "/apple-touch-icon.png", sizes: "180x180", type: "image/png" }],
  },
};

export const viewport: Viewport = {
  width: "device-width",
  initialScale: 1,
  themeColor: [
    { media: "(prefers-color-scheme: light)", color: "#faf8f5" },
    { media: "(prefers-color-scheme: dark)", color: "#2a2724" },
  ],
};

// Runs before first paint so a stored dark theme, collapsed sidebar or reading width never flashes.
// Keys mirror lib/prefs.ts.
const BOOT = `(function(){try{var d=document.documentElement,s=localStorage,t=s.getItem('libra:theme')||'light';d.dataset.themePref=t;d.dataset.theme=t==='system'?(matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light'):t;var b=s.getItem('libra:sidebar');if(b)d.dataset.sidebar=b;var f=s.getItem('libra:face');if(f==='sans'||f==='mono'||f==='serif')d.dataset.face=f;var c=s.getItem('libra:col');if(c&&c!=='0')d.style.setProperty('--col',c+'rem');var z=parseFloat(s.getItem('libra:size'));if(z>=13&&z<=22&&z!==15)d.style.setProperty('--ui-size',z+'px');var l=parseFloat(s.getItem('libra:leading'));if(l>=1.3&&l<=1.85&&l!==1.5)d.style.setProperty('--ui-leading',String(l));var g=(s.getItem('libra:font')||'').replace(/[^A-Za-z0-9 ,.'"-]/g,'').trim();if(g){d.dataset.font='custom';d.style.setProperty('--font-custom',g);}var chrome=d.dataset.theme==='dark'?'#2a2724':'#faf8f5';document.querySelectorAll('meta[name="theme-color"]').forEach(function(m){m.setAttribute('content',chrome);m.removeAttribute('media');});}catch(e){}})();`;

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <title>Libra</title>
        <script dangerouslySetInnerHTML={{ __html: BOOT }} />
      </head>
      <body>
        <Pwa />
        <Shell>{children}</Shell>
      </body>
    </html>
  );
}
