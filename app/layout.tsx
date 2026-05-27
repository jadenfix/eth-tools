import './globals.css';
import type { ReactNode } from 'react';

export const metadata = {
  title: 'eth-tools — runtime for ERC-8004 agents',
  description:
    'Agent-first runtime for the ERC-8004 trustless-agent registry. HTTP API · MCP server · CLI. Autonomous Rust workers keep the data plane fresh.',
  applicationName: 'eth-tools',
  authors: [{ name: 'jadenfix', url: 'https://github.com/jadenfix' }],
  keywords: [
    'ERC-8004',
    'trustless agents',
    'MCP',
    'Model Context Protocol',
    'agent registry',
    'eth-tools',
    'Base',
    'x402',
  ],
  openGraph: {
    title: 'eth-tools — runtime for ERC-8004 agents',
    description:
      'Agent-first runtime for the ERC-8004 trustless-agent registry. HTTP API · MCP server · CLI.',
    type: 'website',
    url: 'https://eth-tools.dev',
  },
  twitter: {
    card: 'summary_large_image',
    title: 'eth-tools',
    description: 'Runtime for ERC-8004 trustless agents — for other agents.',
  },
  icons: {
    icon: 'data:image/svg+xml,%3Csvg xmlns=%22http://www.w3.org/2000/svg%22 viewBox=%220 0 32 32%22%3E%3Crect width=%2232%22 height=%2232%22 rx=%226%22 fill=%22%2307030f%22/%3E%3Ctext x=%2216%22 y=%2222%22 font-family=%22monospace%22 font-size=%2218%22 font-weight=%22700%22 fill=%22%23bb7dff%22 text-anchor=%22middle%22%3E%24%3C/text%3E%3C/svg%3E',
  },
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" className="dark">
      <body className="min-h-screen antialiased">{children}</body>
    </html>
  );
}
