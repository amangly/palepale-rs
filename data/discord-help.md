palepale discord setup --owner YOUR_DISCORD_USER_ID
palepale discord run
palepale discord status
palepale discord logout

Setup reads DISCORD_BOT_TOKEN or asks for it without echoing.
Add --allow-user ID or --allow-role ID during setup to grant access.
Add --free-channel ID to answer without mentions in that channel.
Add --thread to start a thread for channel mentions.
Pick a model with palepale --model MODEL_ID discord setup --owner USER_ID.

Enable Message Content Intent in the Discord Developer Portal.
Invite the bot with bot and applications.commands scopes.
Use /palepale ask, model, reset, stop, usage or help in Discord.
Session history stays in memory until reset or exit. The host account pays for usage.
