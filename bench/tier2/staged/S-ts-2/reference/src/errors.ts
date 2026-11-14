// Errors raised while parsing and while rendering templates, with the place where they happened.

export type ErrorLocation = { template?: string | null; line?: number | null };

export class TemplateError extends Error {
  template: string | null;
  line: number | null;

  constructor(message: string, location: ErrorLocation = {}) {
    super(message);
    this.name = new.target.name;
    this.template = location.template ?? null;
    this.line = location.line ?? null;
  }

  // The message prefixed by the template name and line, as far as they are known.
  describe(): string {
    if (this.template !== null && this.line !== null) return `${this.template}:${this.line}: ${this.message}`;
    if (this.template !== null) return `${this.template}: ${this.message}`;
    if (this.line !== null) return `line ${this.line}: ${this.message}`;
    return this.message;
  }
}

export class TemplateSyntaxError extends TemplateError {
  declare line: number;

  constructor(message: string, location: { template?: string | null; line: number }) {
    super(message, location);
  }
}

export class RenderError extends TemplateError {}
