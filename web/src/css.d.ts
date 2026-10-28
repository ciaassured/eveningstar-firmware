declare module "*.module.css" {
  const classes: Readonly<Record<string, string>>;
  export default classes;
}

// Plain style sheets are imported for their effect.
declare module "*.css";
