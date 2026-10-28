/** How the transport authenticates its requests. */
export interface AuthProvider {
  /** The request options with the credentials added. */
  decorate(init: RequestInit): RequestInit;
  /**
   * Called when the device answers 401: `"retry"` if the credentials were
   * renewed, `"login-required"` if the person has to sign in.
   */
  onUnauthorized(): Promise<"retry" | "login-required">;
}
