import './globals.css';

export const metadata = {
  title: 'eth-tools',
  description: 'Self-maintaining runtime for ERC-8004 trustless agents.',
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
