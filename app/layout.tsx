import './globals.css';
import type { ReactNode } from 'react';

export const metadata = {
  title: 'eth-tools',
  description: 'Self-maintaining runtime for ERC-8004 trustless agents.',
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" className="dark">
      <body className="min-h-screen bg-[#0b0d10] text-zinc-100 antialiased">
        {children}
      </body>
    </html>
  );
}
