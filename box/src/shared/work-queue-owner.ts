/** Your own queue's key: the items you added from a client that are no conversation's (yet). */
export const YOURS = "usr_you";
/** An owner's key: a conversation's address, a group chat's id, or your own queue's. */
export const OWNER = /^(?:ses_[A-Za-z0-9]{1,60}|grp_[a-z0-9]{1,60}|usr_you)$/;
