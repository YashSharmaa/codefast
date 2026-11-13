import React, { useCallback, useState, type FC } from 'react';
import * as Icons from './icons';
import { Button as Btn, List } from './ui';
import styles from './view.module.css';

interface Props { items: Item[]; onSelect?: (item: Item) => void; title: string }
type State = { count: number; selected?: Item };
const theme = { dark: false } satisfies Theme;
const initial = { count: 0 } as State;

export const TodoList: FC<Props> = ({ items, onSelect, title = 'Todo' }) => {
	const [filter, setFilter] = useState<string>('');
	const handleAdd = useCallback(() => { addItem(filter); }, [filter]);
	const handleRemove = (id: number) => removeItem(id);
	function renderRow(item: Item) {
		return <li key={item.id} className={styles.row} onClick={() => onSelect?.(item)}>{item.label}</li>;
	}
	if (!items.length) {
		return <Empty />;
	}
	return (
		<List<Item> header={<Header title={title} />} data={items} {...theme}>
			<Btn onClick={handleAdd} disabled={!filter}>Add</Btn>
			<Icons.Trash size={12} onClick={handleRemove} />
			<input value={filter} onChange={e => setFilter(e.target.value)} />
			{items.map(renderRow)}
			<>
				<React.Fragment key="x" />
			</>
		</List>
	);
};

function Row({ item, onPick, children }: RowProps): JSX.Element {
	const pick = () => onPick(item.id);
	return <Btn onClick={pick}>{children}</Btn>;
}

export default function App(props: { user: User }) {
	const { user } = props;
	const service = useService(IUserService);
	return <TodoList items={user.items} title="t" onSelect={service.select} />;
}

@observer
export class Counter extends React.Component<Props, State> {
	state: State = { count: 0 };
	private timer?: Timer;
	render() {
		const { title, items } = this.props;
		return <div onClick={this.handleClick} ref={this.attach}>{title}{this.state.count}{items.length}</div>;
	}
	handleClick = () => { this.setState({ count: this.state.count + 1 }); };
	private attach(el: HTMLDivElement | null) { this.timer = new Timer(el!); this.timer.start(); }
}
