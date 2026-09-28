#[path = "models/base.rs"]
mod base;
#[path = "models/rich_text.rs"]
mod rich_text;

#[allow(unused_imports)]
pub use base::{
    deserialize_flexible_i32, deserialize_flexible_i64, deserialize_flexible_opt_i32,
    deserialize_flexible_opt_i64, validate_quiz, ApiResponse, Audio, BotCommand, CallbackQuery,
    Chat, ChatMember, CopyTextButton, Document, EphemeralMessageParameters, FileInfo,
    InlineKeyboardButton, InlineKeyboardMarkup, InputMedia, InputPollOption, InputRichMessage,
    InputRichMessageMedia, LivePhoto, Location, LoginUrl, Message, MessageGenerationStopped,
    PhotoSize, Poll, PollOption, ReplyParameters, RichBlock, RichBlockCaption, RichBlockListItem,
    RichBlockTableCell, RichMessageButton, RichTextButton, SentGuestMessage, StagedDocument,
    Sticker, SwitchInlineQueryChosenChat, Update, User, Venue, Video, VideoNote, Voice,
    QUIZ_MAX_EXPLANATION_CHARS, QUIZ_MAX_EXPLANATION_LINE_BREAKS, QUIZ_MAX_OPTIONS,
    QUIZ_MAX_OPTION_CHARS, QUIZ_MAX_QUESTION_CHARS, QUIZ_MIN_OPTIONS, RICH_MESSAGE_MAX_BLOCKS,
    RICH_MESSAGE_MAX_BUTTONS_PER_ROW, RICH_MESSAGE_MAX_MEDIA, RICH_MESSAGE_MAX_NESTING,
    RICH_MESSAGE_MAX_TABLE_COLUMNS, RICH_MESSAGE_MAX_TEXT_CHARS,
};
