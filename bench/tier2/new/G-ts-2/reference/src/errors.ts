// Errors raised while parsing and while rendering templates.

export class TemplateError extends Error {
  constructor(message: string) {
    super(message);
    this.name = new.target.name;
  }
}

export class TemplateSyntaxError extends TemplateError {
  line: number;

  constructor(message: string, line: number) {
    super(message);
    this.line = line;
  }
}

export class RenderError extends TemplateError {}
