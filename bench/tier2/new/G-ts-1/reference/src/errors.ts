// Error classes thrown by the scheduler and mapped to HTTP status codes by the handler.

export class SchedulerError extends Error {
  constructor(message: string) {
    super(message);
    this.name = new.target.name;
  }
}

export class ValidationError extends SchedulerError {}

export class NotFoundError extends SchedulerError {}

export class NoRoomError extends SchedulerError {}

export class ConflictError extends SchedulerError {
  conflicts: string[];

  constructor(message: string, conflicts: string[] = []) {
    super(message);
    this.conflicts = conflicts;
  }
}
