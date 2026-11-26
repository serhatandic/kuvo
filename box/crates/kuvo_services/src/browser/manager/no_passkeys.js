(() => {
  try {
    // Says a passkey was asked for with an event only this launch's listener, in a world of its
    // own, knows the name of: nothing of it is left in the page's world. (Taken before the page
    // can change them.)
    const dispatch = EventTarget.prototype.dispatchEvent, Made = Event;
    const tell = () => {
      try {
        dispatch.call(window, new Made("KUVO_PASSKEY_EVENT"));
      } catch {}
    };
    const none = (options) => {
      // Asked of you, not looked for quietly as the page loads.
      if (options.mediation !== "conditional") tell();
      return Promise.reject(new DOMException("Passkeys can't be used in this browser.", "NotAllowedError"));
    };
    const Container = window.CredentialsContainer;
    if (Container && Container.prototype) {
      const prototype = Container.prototype, get = prototype.get, create = prototype.create;
      if (get) prototype.get = function (options) { return options && options.publicKey ? none(options) : get.call(this, options); };
      if (create) prototype.create = function (options) { return options && options.publicKey ? none(options) : create.call(this, options); };
    }
    const Credential = window.PublicKeyCredential;
    if (Credential) {
      Credential.isUserVerifyingPlatformAuthenticatorAvailable = () => Promise.resolve(false);
      if (Credential.isConditionalMediationAvailable) Credential.isConditionalMediationAvailable = () => Promise.resolve(false);
    }
  } catch {}
})();
