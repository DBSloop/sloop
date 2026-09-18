-- The schedule the service keeps, per attached database — `R27a`.
--
-- **This is what makes a cron line unnecessary, which is the whole point of the entry.** The
-- three things `backups prune` already takes — how often, how many to keep, how old is too old
-- — stored against the database rather than typed into a crontab. A machine with sloop
-- installed and a database attached has working backups without anybody editing a system file.
--
-- On the attachment rather than in a table of their own, for the reason `0008`'s baseline
-- columns are: there is exactly one schedule per attachment, and detaching should forget it.
-- Re-attaching starts from nothing rather than resuming a schedule nobody remembers setting.

-- How often, in seconds. NULL is the ordinary state: attached and sampled, and nobody has
-- asked for backups.
ALTER TABLE monitored_database ADD COLUMN backup_every_seconds BIGINT;

-- The retention policy, exactly as `backups prune` means them. Both NULL is "keep everything",
-- which is what prune refuses to run with -- so a schedule with no policy takes backups and
-- prunes nothing, rather than deleting on a rule nobody set.
ALTER TABLE monitored_database ADD COLUMN keep_last INTEGER;
ALTER TABLE monitored_database ADD COLUMN keep_for_days INTEGER;

-- When the next one is due, and when the last one actually ran.
--
-- **Both, because they answer different questions**, and `R27a` asks for both on the screen:
-- rule 14 says the schedule is checked rather than trusted, so a schedule that has silently
-- stopped is visible without waiting for the backup nobody took.
ALTER TABLE monitored_database ADD COLUMN backup_due_at TIMESTAMPTZ;
ALTER TABLE monitored_database ADD COLUMN backup_ran_at TIMESTAMPTZ;

-- What happened, as the exit code and a word.
--
-- `7` is the one worth naming: a scheduled run that collided with a manual one did not fail,
-- it stood aside. `6` is finished-but-the-counts-disagreed. Keeping the code rather than a
-- boolean is what lets the screen say which.
ALTER TABLE monitored_database ADD COLUMN backup_exit_code INTEGER;
ALTER TABLE monitored_database ADD COLUMN backup_outcome TEXT;

-- Whether that run was later than it should have been.
--
-- **A laptop asleep at 03:00 backs up when it wakes, and says it was late.** Pretending it was
-- on time is the one thing a backup schedule must not do, because the whole reason to look at
-- one is to find out whether it is keeping up.
ALTER TABLE monitored_database ADD COLUMN backup_was_late BOOLEAN;
