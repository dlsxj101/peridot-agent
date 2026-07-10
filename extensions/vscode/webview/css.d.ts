// Ambient declaration for side-effect CSS imports (e.g. `import './style.css'`).
// esbuild handles the actual CSS bundling; this only satisfies the TypeScript
// compiler (TS 6.0+ requires a type for such module specifiers — TS2882).
declare module '*.css';
