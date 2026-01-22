import { motion } from 'framer-motion';
import { MessageSquare, Clock } from 'lucide-react';
import { useUnifiedHaloStore } from '@/stores/unifiedHaloStore';
import type { HaloTopic } from '@/stores/unifiedHaloStore';

function formatRelativeTime(timestamp: number): string {
  const now = Date.now();
  const diff = now - timestamp;
  const minutes = Math.floor(diff / 60000);
  const hours = Math.floor(diff / 3600000);
  const days = Math.floor(diff / 86400000);

  if (minutes < 1) return 'Just now';
  if (minutes < 60) return `${minutes}m ago`;
  if (hours < 24) return `${hours}h ago`;
  return `${days}d ago`;
}

interface TopicItemProps {
  topic: HaloTopic;
  isSelected: boolean;
  onClick: () => void;
}

function TopicItem({ topic, isSelected, onClick }: TopicItemProps) {
  return (
    <button
      onClick={onClick}
      className={`w-full flex items-center gap-3 px-3 py-2 rounded-md transition-colors text-left ${
        isSelected
          ? 'bg-primary/10 text-primary'
          : 'hover:bg-secondary/80 text-foreground'
      }`}
    >
      <MessageSquare className="w-4 h-4 text-muted-foreground flex-shrink-0" />
      <div className="flex-1 min-w-0">
        <div className="font-medium text-sm truncate">{topic.title}</div>
        <div className="flex items-center gap-1 text-xs text-muted-foreground">
          <Clock className="w-3 h-3" />
          {formatRelativeTime(topic.updatedAt)}
        </div>
      </div>
    </button>
  );
}

interface TopicListProps {
  maxHeight?: number;
}

export function TopicList({ maxHeight = 300 }: TopicListProps) {
  const { filteredTopics, selectedTopicIndex, selectTopic } =
    useUnifiedHaloStore();

  if (filteredTopics.length === 0) {
    return (
      <div className="px-3 py-6 text-center text-sm text-muted-foreground">
        No topics found
      </div>
    );
  }

  return (
    <motion.div
      initial={{ opacity: 0, height: 0 }}
      animate={{ opacity: 1, height: 'auto' }}
      exit={{ opacity: 0, height: 0 }}
      transition={{ duration: 0.15 }}
      className="overflow-hidden"
    >
      <div
        className="overflow-y-auto py-1 px-1"
        style={{ maxHeight }}
      >
        {filteredTopics.map((topic, index) => (
          <TopicItem
            key={topic.id}
            topic={topic}
            isSelected={index === selectedTopicIndex}
            onClick={() => selectTopic(topic)}
          />
        ))}
      </div>
    </motion.div>
  );
}
