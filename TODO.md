# Roon Remote Display - TODO List

## Future Investigations and Improvements

### Roon API Event Handling
1. **Make sure we log all `Parsed::Error` events as INFO in the server**
   - Currently these errors might not be logged properly
   - Need to add explicit logging for API errors

2. **Find out what `Parsed::Outputs(Vec<Output>)` means**
   - This event is currently not handled in our code
   - Need to understand when it's sent and what data it contains
   - Determine if we should react to it

3. **Handle `Parsed::Queue(Vec<QueueItem>)` and `Parsed::QueueChanges(Vec<QueueChange>)`**
   - Currently not handled in our code
   - Need to determine when these are sent from Roon
   - Understand the relationship to our current queue subscription
   - Implement proper handling after understanding the timing/purpose

4. **Determine what the `Parsed::SettingsSubscribed/Unsubscribed/Saved` events and the Settings feature in general is**
   - Settings feature is currently not enabled in our Cargo.toml dependencies
   - Need to understand what Roon settings this exposes
   - Determine if we want to enable this feature and how to use it

## Performance and UI Issues

### High Priority - Fix Zone Re-rendering
- **Problem**: `Parsed::ZonesSeek` (sent every ~1 second during playback) triggers full zone re-render
- **Symptoms**:
  - Queue button flashing when hovering
  - Queue popup disappearing after double-click
  - Poor performance with full DOM rebuild every second
- **Solution**: Add separate WebSocket message type for seek updates that only updates progress bars
  - Consider comparing old vs new data before updating
  - Use DOM manipulation instead of `innerHTML =`

## Completed
- ✅ Version 1.0.0 - Initial release with queue display
- ✅ Version 1.1.0 - UI improvements and reconnect functionality
- ✅ Version 1.2.0 - Fullscreen support and responsive design
