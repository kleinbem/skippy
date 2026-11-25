import Link from "next/link";

export default function NotFound() {
  return (
    <main className="skippy-not-found">
      <p className="skippy-not-found__eyebrow">404</p>
      <h1>Page not found</h1>
      <p>The documentation may have moved, or the address may be incomplete.</p>
      <Link href="/">Return to the Skippy docs</Link>
    </main>
  );
}
