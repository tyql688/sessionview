declare module "*?raw" {
  const content: string;
  export default content;
}

// TS 6 flags side-effect imports without declarations (TS2882).
declare module "*.css";
